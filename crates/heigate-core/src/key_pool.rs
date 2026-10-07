//! A short selection lock protects round-robin position and recovery leases.
//! No network I/O runs under this lock. Generations fence late in-flight replies.
use crate::{
    resilience::{failure_cooldown, FailureClass, FailureInfo},
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

    pub fn acquire_for_probe(
        self: &Arc<Self>,
        channel: i64,
        _model: &str,
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
            // For probe: bypass cooldown_until and channel_key_models so diagnostics can always test!
            if !key.enabled || key.auth_invalid || specific.is_some_and(|id| id != key.id) {
                continue;
            }
            let updated = self.store.lock().execute(
                "UPDATE channel_keys SET attempts=attempts+1,last_used_at=? WHERE id=? AND generation=? AND enabled=1 AND auth_invalid=0",
                params![now.to_rfc3339(), key.id, key.generation],
            );
            if !matches!(updated, Ok(1)) {
                updated?;
                continue;
            }
            state.last.insert(channel, key.id);
            return Ok(Some(KeyLease {
                key: key.clone(),
                pool: self.clone(),
                recovery: false,
            }));
        }
        Ok(None)
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
            {
                continue;
            }
            if !model.is_empty() {
                let unavailable: bool = self.store.lock().query_row(
                    "SELECT EXISTS(SELECT 1 FROM channel_key_models WHERE key_id=? AND model=? AND unavailable_until>?)",
                    params![key.id, model, now.to_rfc3339()],
                    |r| r.get(0),
                )?;
                if unavailable {
                    continue;
                }
            }
            if key.cooldown_until.is_some_and(|until| until > now) {
                continue;
            }
            let recovery = key.cooldown_until.is_some();
            if recovery && !state.recovery.insert((key.id, key.generation)) {
                continue;
            }
            let updated = self.store.lock().execute("UPDATE channel_keys SET attempts=attempts+1,last_used_at=? WHERE id=? AND generation=? AND enabled=1 AND auth_invalid=0",params![now.to_rfc3339(),key.id,key.generation]);
            if !matches!(updated, Ok(1)) {
                if recovery {
                    state.recovery.remove(&(key.id, key.generation));
                }
                updated?;
                continue;
            }
            state.last.insert(channel, key.id);
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

    pub fn retry_after_model(&self, channel: i64, model: &str) -> Duration {
        if model.is_empty() {
            return self.retry_after(channel);
        }
        let keys = self.store.list_keys(channel).unwrap_or_default();
        let now = Utc::now();
        let mut min_wait: Option<Duration> = None;
        {
            let conn = self.store.lock();
            for k in keys.iter().filter(|k| k.enabled && !k.auth_invalid) {
                let row: rusqlite::Result<String> = conn.query_row(
                    "SELECT unavailable_until FROM channel_key_models WHERE key_id=? AND model=? AND unavailable_until>?",
                    params![k.id, model, now.to_rfc3339()],
                    |r| r.get(0),
                );
                if let Ok(until_str) = row {
                    if let Ok(until) = chrono::DateTime::parse_from_rfc3339(&until_str) {
                        if let Ok(d) = (until.with_timezone(&Utc) - now).to_std() {
                            min_wait = Some(min_wait.map_or(d, |curr: Duration| curr.min(d)));
                        }
                    }
                }
            }
        }
        min_wait.unwrap_or_default()
    }
}

/// Upper bound for any persisted cooldown. Cooldown deadlines are stored as
/// RFC3339 text and compared as strings in SQL, so they must stay within the
/// four-digit-year range (e.g. never `DateTime::MAX_UTC`, whose `+262142-…`
/// form sorts before `2026-…` and would look already expired).
const MAX_COOLDOWN: Duration = Duration::from_secs(7 * 24 * 3600);

pub fn rate_limit_cooldown(count: i64, upstream: Duration) -> Duration {
    let seconds = match count {
        i64::MIN..=1 => 300,
        2 => 600,
        3 => 1200,
        _ => 1800,
    };
    Duration::from_secs(seconds).max(upstream).min(MAX_COOLDOWN)
}

impl KeyLease {
    pub fn failure(&self, info: &FailureInfo, model: &str) -> Result<()> {
        let mut conn = self.pool.store.lock();
        let tx = conn.transaction()?;
        let now = Utc::now();
        match info.class {
            Some(FailureClass::RateLimit) => {
                if model.is_empty() {
                    tx.commit()?;
                    return Ok(());
                }
                let count: i64 = tx
                    .query_row(
                        "SELECT rate_limit_count FROM channel_key_models WHERE key_id=? AND model=?",
                        params![self.key.id, model],
                        |row| row.get(0),
                    )
                    .unwrap_or(0);
                let delay = rate_limit_cooldown(count + 1, info.retry_after);
                let until = chrono::Duration::from_std(delay)
                    .ok()
                    .and_then(|duration| now.checked_add_signed(duration))
                    .unwrap_or(chrono::DateTime::<Utc>::MAX_UTC);
                tx.execute(
                    "INSERT INTO channel_key_models(key_id,model,unavailable_until,rate_limit_count) \
                     SELECT id,?,?,1 FROM channel_keys WHERE id=? AND generation=? \
                     ON CONFLICT(key_id,model) DO UPDATE SET \
                       unavailable_until=excluded.unavailable_until, \
                       rate_limit_count=channel_key_models.rate_limit_count+1",
                    params![model, until.to_rfc3339(), self.key.id, self.key.generation],
                )?;
                tx.execute(
                    "UPDATE channel_keys SET rate_limits=rate_limits+1,last_error='429：限流冷却',generation=generation+1 WHERE id=? AND generation=?",
                    params![self.key.id, self.key.generation],
                )?;
            }
            Some(FailureClass::Auth) if info.status_code == 401 => {
                tx.execute(
                    "UPDATE channel_keys SET auth_invalid=1,last_error='401：认证不可用，请替换密钥',generation=generation+1 WHERE id=? AND generation=?",
                    params![self.key.id, self.key.generation],
                )?;
            }
            Some(FailureClass::Auth | FailureClass::Model) => {
                let until = (now + chrono::Duration::minutes(2)).to_rfc3339();
                if !model.is_empty() {
                    tx.execute(
                        "INSERT INTO channel_key_models(key_id,model,unavailable_until) \
                         SELECT id,?,? FROM channel_keys WHERE id=? AND generation=? \
                         ON CONFLICT(key_id,model) DO UPDATE SET unavailable_until=excluded.unavailable_until",
                        params![model, until, self.key.id, self.key.generation],
                    )?;
                }
                tx.execute(
                    "UPDATE channel_keys SET generation=generation+1 WHERE id=? AND generation=?",
                    params![self.key.id, self.key.generation],
                )?;
            }
            Some(FailureClass::Provider | FailureClass::Network) => {
                let delay = failure_cooldown(info, 1);
                let until = chrono::Duration::from_std(delay)
                    .ok()
                    .and_then(|duration| now.checked_add_signed(duration))
                    .unwrap_or(chrono::DateTime::<Utc>::MAX_UTC);
                if !model.is_empty() {
                    tx.execute(
                        "INSERT INTO channel_key_models(key_id,model,unavailable_until) \
                         SELECT id,?,? FROM channel_keys WHERE id=? AND generation=? \
                         ON CONFLICT(key_id,model) DO UPDATE SET unavailable_until=excluded.unavailable_until",
                        params![model, until.to_rfc3339(), self.key.id, self.key.generation],
                    )?;
                }
            }
            _ => {}
        }
        tx.commit()?;
        Ok(())
    }

    pub fn success(&self, prompt: i64, completion: i64, model: &str) -> Result<()> {
        let mut conn = self.pool.store.lock();
        let tx = conn.transaction()?;
        let updated = tx.execute("UPDATE channel_keys SET successes=successes+1,prompt_tokens=prompt_tokens+?,completion_tokens=completion_tokens+?,last_success_at=?,last_error='' WHERE id=? AND generation=?",params![prompt.max(0),completion.max(0),Utc::now().to_rfc3339(),self.key.id,self.key.generation])?;
        if updated == 1 && !model.is_empty() {
            tx.execute(
                "DELETE FROM channel_key_models WHERE key_id=? AND model=?",
                params![self.key.id, model],
            )?;
        }
        tx.commit()?;
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
            .add_key(
                ch.id,
                &KeyInput {
                    name: "B".into(),
                    secret: "key-b".into(),
                },
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
    fn rate_limit_is_fenced_and_scoped_to_model() {
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
        stale.success(1, 2, "other-model").unwrap();
        let fresh = pool.store.list_keys(id).unwrap().remove(0);
        assert_eq!(fresh.rate_limit_count, 0);
        assert!(fresh.cooldown_until.is_none());
        assert!(pool
            .acquire(id, "m", &HashSet::new(), Some(a.key.id))
            .unwrap()
            .is_none());
        assert!(pool
            .acquire(id, "other-model", &HashSet::new(), Some(a.key.id))
            .unwrap()
            .is_some());
        assert_eq!(pool.retry_after_model(id, "other-model"), Duration::ZERO);
        assert!(pool.retry_after_model(id, "m") >= Duration::from_secs(290));
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

    #[test]
    fn extreme_retry_after_does_not_overflow_or_reenable_key() {
        let (pool, id) = setup();
        let lease = pool
            .acquire(id, "m", &HashSet::new(), None)
            .unwrap()
            .unwrap();
        lease
            .failure(
                &FailureInfo {
                    class: Some(FailureClass::RateLimit),
                    status_code: 429,
                    retry_after: Duration::MAX,
                },
                "m",
            )
            .unwrap();
        assert!(pool
            .acquire(id, "m", &HashSet::new(), Some(lease.key.id))
            .unwrap()
            .is_none());
    }
}
