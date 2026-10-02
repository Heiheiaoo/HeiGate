//! A short selection lock protects round-robin position and recovery leases.
//! No network I/O runs under this lock. Generations fence late in-flight replies.
use crate::{
    resilience::{FailureClass, FailureInfo},
    store::{Result, Store},
    store_keys::ChannelKey,
};
use chrono::Utc;
use rusqlite::params;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Default)]
struct State {
    last: HashMap<i64, i64>,
    recovery: HashSet<(i64, i64)>,
}

pub struct KeyPool {
    store: Arc<Store>,
    state: Mutex<State>,
}

pub struct KeyLease {
    pub key: ChannelKey,
    pool: Arc<KeyPool>,
    recovery: bool,
}

impl KeyPool {
    pub fn new(store: Arc<Store>) -> Arc<Self> {
        Arc::new(Self {
            store,
            state: Mutex::new(State::default()),
        })
    }

    pub fn acquire(
        self: &Arc<Self>,
        channel: i64,
        model: &str,
        excluded: &HashSet<i64>,
        specific: Option<i64>,
    ) -> Result<Option<KeyLease>> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let keys = self.store.list_keys(channel)?;
        let start = state
            .last
            .get(&channel)
            .and_then(|id| keys.iter().position(|k| k.id == *id))
            .map_or(0, |i| i + 1);
        let now = Utc::now();
        for offset in 0..keys.len() {
            let key = &keys[(start + offset) % keys.len()];
            if !key.enabled
                || key.auth_invalid
                || excluded.contains(&key.id)
                || specific.is_some_and(|id| id != key.id)
                || key.cooldown_until.is_some_and(|until| until > now)
            {
                continue;
            }
            let unavailable:bool=self.store.lock().query_row("SELECT EXISTS(SELECT 1 FROM channel_key_models WHERE key_id=? AND model=? AND unavailable_until>?)",params![key.id,model,now.to_rfc3339()],|r|r.get(0))?;
            if unavailable {
                continue;
            }
            let recovery = key.cooldown_until.is_some();
            if recovery && !state.recovery.insert((key.id, key.generation)) {
                continue;
            }
            state.last.insert(channel, key.id);
            self.store.lock().execute("UPDATE channel_keys SET attempts=attempts+1,last_used_at=? WHERE id=? AND generation=?",params![now.to_rfc3339(),key.id,key.generation])?;
            return Ok(Some(KeyLease {
                key: key.clone(),
                pool: self.clone(),
                recovery,
            }));
        }
        Ok(None)
    }

    pub fn retry_after(&self, channel: i64) -> Duration {
        self.store
            .list_keys(channel)
            .unwrap_or_default()
            .iter()
            .filter(|k| k.enabled && !k.auth_invalid)
            .filter_map(|k| k.cooldown_until)
            .filter_map(|until| (until - Utc::now()).to_std().ok())
            .min()
            .unwrap_or_default()
    }
}

pub fn rate_limit_cooldown(count: i64, upstream: Duration) -> Duration {
    let seconds = match count {
        i64::MIN..=1 => 300,
        2 => 600,
        3 => 1200,
        _ => 1800,
    };
    Duration::from_secs(seconds).max(upstream)
}

impl KeyLease {
    pub fn failure(&self, info: &FailureInfo, model: &str) -> Result<()> {
        let mut conn = self.pool.store.lock();
        let tx = conn.transaction()?;
        let now = Utc::now();
        match info.class {
            Some(FailureClass::RateLimit) => {
                let delay = rate_limit_cooldown(self.key.rate_limit_count + 1, info.retry_after);
                let until =
                    now + chrono::Duration::from_std(delay).unwrap_or(chrono::Duration::days(365));
                tx.execute("UPDATE channel_keys SET cooldown_until=?,rate_limit_count=rate_limit_count+1,rate_limits=rate_limits+1,last_error='429：限流冷却',generation=generation+1 WHERE id=? AND generation=?",params![until.to_rfc3339(),self.key.id,self.key.generation])?;
            }
            Some(FailureClass::Auth) if info.status_code == 401 => {
                tx.execute("UPDATE channel_keys SET auth_invalid=1,last_error='401：认证不可用，请替换密钥',generation=generation+1 WHERE id=? AND generation=?",params![self.key.id,self.key.generation])?;
            }
            Some(FailureClass::Auth | FailureClass::Model) => {
                let until = (now + chrono::Duration::minutes(2)).to_rfc3339();
                tx.execute("INSERT INTO channel_key_models(key_id,model,unavailable_until) SELECT id,?,? FROM channel_keys WHERE id=? AND generation=? ON CONFLICT(key_id,model) DO UPDATE SET unavailable_until=excluded.unavailable_until",params![model,until,self.key.id,self.key.generation])?;
                tx.execute("UPDATE channel_keys SET last_error='模型权限或端点错误：该模型暂避 2 分钟' WHERE id=? AND generation=?",params![self.key.id,self.key.generation])?;
            }
            _ => {}
        }
        tx.commit()?;
        Ok(())
    }

    pub fn success(&self, prompt: i64, completion: i64) -> Result<()> {
        self.pool.store.lock().execute("UPDATE channel_keys SET successes=successes+1,prompt_tokens=prompt_tokens+?,completion_tokens=completion_tokens+?,last_success_at=?,cooldown_until=NULL,rate_limit_count=0,last_error='' WHERE id=? AND generation=?",params![prompt.max(0),completion.max(0),Utc::now().to_rfc3339(),self.key.id,self.key.generation])?;
        Ok(())
    }
}

impl Drop for KeyLease {
    fn drop(&mut self) {
        if self.recovery {
            self.pool
                .state
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .recovery
                .remove(&(self.key.id, self.key.generation));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{store::Channel, store_keys::KeyInput};
    fn setup() -> (Arc<KeyPool>, i64) {
        let store = Store::open_in_memory().unwrap();
        let mut ch = Channel {
            api_key: "key-a".into(),
            ..Default::default()
        };
        store.save(&mut ch).unwrap();
        store
            .add_keys(
                ch.id,
                &[KeyInput {
                    name: "B".into(),
                    secret: "key-b".into(),
                }],
            )
            .unwrap();
        (KeyPool::new(store), ch.id)
    }
    #[test]
    fn polling_and_exclusion() {
        let (pool, id) = setup();
        let a = pool
            .acquire(id, "model", &HashSet::new(), None)
            .unwrap()
            .unwrap();
        let b = pool
            .acquire(id, "model", &HashSet::new(), None)
            .unwrap()
            .unwrap();
        assert_ne!(a.key.id, b.key.id);
        assert_eq!(
            pool.acquire(id, "model", &HashSet::new(), None)
                .unwrap()
                .unwrap()
                .key
                .id,
            a.key.id
        );
        assert!(pool
            .acquire(id, "model", &HashSet::from([a.key.id, b.key.id]), None)
            .unwrap()
            .is_none());
    }
    #[test]
    fn cooldown_is_fenced_and_never_bypassed() {
        let (pool, id) = setup();
        let a = pool
            .acquire(id, "m", &HashSet::new(), None)
            .unwrap()
            .unwrap();
        let stale = pool
            .acquire(id, "m", &HashSet::new(), Some(a.key.id))
            .unwrap()
            .unwrap();
        a.failure(&FailureInfo::new(FailureClass::RateLimit), "m")
            .unwrap();
        stale
            .failure(&FailureInfo::new(FailureClass::RateLimit), "m")
            .unwrap();
        stale.success(1, 2).unwrap();
        let fresh = pool.store.list_keys(id).unwrap().remove(0);
        assert_eq!(fresh.rate_limit_count, 1);
        assert!(fresh.cooldown_until.unwrap() > Utc::now() + chrono::Duration::seconds(290));
        assert!(pool
            .acquire(id, "m", &HashSet::new(), Some(a.key.id))
            .unwrap()
            .is_none());
    }
    #[test]
    fn recovery_has_one_slot_and_raii_release() {
        let (pool, id) = setup();
        let key = pool.store.list_keys(id).unwrap().remove(0);
        pool.store
            .lock()
            .execute(
                "UPDATE channel_keys SET cooldown_until=? WHERE id=?",
                params![
                    (Utc::now() - chrono::Duration::seconds(1)).to_rfc3339(),
                    key.id
                ],
            )
            .unwrap();
        let lease = pool
            .acquire(id, "m", &HashSet::new(), Some(key.id))
            .unwrap()
            .unwrap();
        assert!(pool
            .acquire(id, "m", &HashSet::new(), Some(key.id))
            .unwrap()
            .is_none());
        drop(lease);
        assert!(pool
            .acquire(id, "m", &HashSet::new(), Some(key.id))
            .unwrap()
            .is_some());
    }
    #[test]
    fn five_minute_backoff_and_upstream_minimum() {
        for (count, secs) in [(1, 300), (2, 600), (3, 1200), (4, 1800), (100, 1800)] {
            assert_eq!(rate_limit_cooldown(count, Duration::ZERO).as_secs(), secs);
        }
        assert_eq!(
            rate_limit_cooldown(1, Duration::from_secs(7200)).as_secs(),
            7200
        );
    }
}
