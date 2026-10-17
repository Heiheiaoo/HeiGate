//! A short selection lock protects round-robin position and recovery leases.
//! No network I/O runs under this lock. Generations fence late in-flight replies.
use crate::{
    resilience::{failure_cooldown, FailureClass, FailureInfo},
    store::{Channel, Result, Store},
    store_keys::ChannelKey,
};
use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Default)]
struct State {
    last: HashMap<i64, i64>,
    recovery: HashSet<(i64, i64)>,
    model_recovery: HashSet<(i64, i64, String, i64)>,
}

pub struct KeyPool {
    store: Arc<Store>,
    state: Mutex<State>,
}

pub struct KeyLease {
    pub key: ChannelKey,
    pool: Arc<KeyPool>,
    recovery: bool,
    model_recovery: bool,
    model: String,
    model_generation: i64,
}

impl KeyPool {
    pub fn new(store: Arc<Store>) -> Arc<Self> {
        Arc::new(Self {
            store,
            state: Mutex::new(State::default()),
        })
    }

    /// Select credentials while the endpoint/protocol snapshot is still current.
    pub fn acquire_for_channel(
        self: &Arc<Self>,
        channel: &Channel,
        model: &str,
        excluded: &HashSet<i64>,
        specific: Option<i64>,
    ) -> Result<Option<KeyLease>> {
        let _guard = self
            .store
            .runtime_mu
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if !self.store.channel_config_is_current(channel, model)? {
            return Ok(None);
        }
        self.acquire(channel.id, model, excluded, specific)
    }

    pub fn acquire_probe_for_channel(
        self: &Arc<Self>,
        channel: &Channel,
        model: &str,
        specific: Option<i64>,
    ) -> Result<Option<KeyLease>> {
        let _guard = self
            .store
            .runtime_mu
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if !self.store.channel_config_is_current(channel, "")? {
            return Ok(None);
        }
        self.acquire_for_probe(channel.id, model, specific)
    }

    pub fn acquire_for_probe(
        self: &Arc<Self>,
        channel: i64,
        model: &str,
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
            let model_generation = self.model_generation(key.id, model)?;
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
                model_recovery: false,
                model: model.to_string(),
                model_generation,
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
            let conn = self.store.lock();
            let (model_generation, unavailable, model_recovery): (i64, bool, bool) = conn.query_row(
                "SELECT generation,unavailable_until>?,unavailable_until<>'' FROM channel_key_models WHERE key_id=? AND model=?",
                params![now.to_rfc3339(), key.id, model],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            ).optional()?.unwrap_or((0, false, false));
            if unavailable || key.cooldown_until.is_some_and(|until| until > now) {
                continue;
            }
            let recovery = key.cooldown_until.is_some();
            let model_claim = (key.id, key.generation, model.to_string(), model_generation);
            if state.recovery.contains(&(key.id, key.generation))
                || state.model_recovery.contains(&model_claim)
            {
                continue;
            }
            let updated = conn.execute("UPDATE channel_keys SET attempts=attempts+1,last_used_at=? WHERE id=? AND generation=? AND enabled=1 AND auth_invalid=0",params![now.to_rfc3339(),key.id,key.generation])?;
            if updated != 1 {
                continue;
            }
            if recovery {
                state.recovery.insert((key.id, key.generation));
            }
            if model_recovery {
                state.model_recovery.insert(model_claim);
            }
            state.last.insert(channel, key.id);
            return Ok(Some(KeyLease {
                key: key.clone(),
                pool: self.clone(),
                recovery,
                model_recovery,
                model: model.to_string(),
                model_generation,
            }));
        }
        Ok(None)
    }

    fn model_generation(&self, key: i64, model: &str) -> Result<i64> {
        Ok(self
            .store
            .lock()
            .query_row(
                "SELECT generation FROM channel_key_models WHERE key_id=? AND model=?",
                params![key, model],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(0))
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
    fn credential_is_current(&self, conn: &rusqlite::Connection) -> Result<bool> {
        Ok(conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM channel_keys WHERE id=? AND generation=? AND enabled=1 AND auth_invalid=0)",
            params![self.key.id, self.key.generation], |row| row.get(0),
        )?)
    }

    fn model_is_current(&self, conn: &rusqlite::Connection, model: &str) -> Result<bool> {
        if self.model != model {
            return Ok(false);
        }
        let generation: i64 = conn
            .query_row(
                "SELECT generation FROM channel_key_models WHERE key_id=? AND model=?",
                params![self.key.id, model],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(0);
        Ok(generation == self.model_generation)
    }

    pub fn is_current(&self, model: &str) -> Result<bool> {
        let conn = self.pool.store.lock();
        self.is_current_in(&conn, model)
    }

    pub(crate) fn is_current_in(&self, conn: &rusqlite::Connection, model: &str) -> Result<bool> {
        Ok(self.credential_is_current(conn)? && self.model_is_current(conn, model)?)
    }

    pub fn failure(&self, info: &FailureInfo, model: &str) -> Result<()> {
        let mut conn = self.pool.store.lock();
        let tx = conn.transaction()?;
        if !self.credential_is_current(&tx)? {
            return Ok(());
        }
        // A 401 invalidates the credential across models. Other failures are
        // fenced only by this model's epoch, not by another model's outcome.
        if info.class == Some(FailureClass::Auth) && info.status_code == 401 {
            tx.execute(
                "UPDATE channel_keys SET auth_invalid=1,last_error='401：认证不可用，请替换密钥',generation=generation+1 WHERE id=? AND generation=?",
                params![self.key.id, self.key.generation],
            )?;
            tx.commit()?;
            return Ok(());
        }
        if model.is_empty() || !self.model_is_current(&tx, model)? {
            return Ok(());
        }
        let now = Utc::now();
        let count: i64 = tx
            .query_row(
                "SELECT rate_limit_count FROM channel_key_models WHERE key_id=? AND model=?",
                params![self.key.id, model],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(0);
        let delay = match info.class {
            Some(FailureClass::RateLimit) => rate_limit_cooldown(count + 1, info.retry_after),
            Some(FailureClass::Auth | FailureClass::Model) => Duration::from_secs(120),
            Some(FailureClass::Provider | FailureClass::Network) => {
                failure_cooldown(info, 1).min(MAX_COOLDOWN)
            }
            _ => return Ok(()),
        };
        let until = now + chrono::Duration::from_std(delay).expect("bounded cooldown");
        let next_count = if info.class == Some(FailureClass::RateLimit) {
            count + 1
        } else {
            count
        };
        tx.execute(
            "INSERT INTO channel_key_models(key_id,model,unavailable_until,rate_limit_count,generation) VALUES(?,?,?,?,?) \
             ON CONFLICT(key_id,model) DO UPDATE SET unavailable_until=excluded.unavailable_until, \
             rate_limit_count=excluded.rate_limit_count,generation=excluded.generation",
            params![self.key.id,model,until.to_rfc3339(),next_count,self.model_generation + 1],
        )?;
        if info.class == Some(FailureClass::RateLimit) {
            tx.execute("UPDATE channel_keys SET rate_limits=rate_limits+1,last_error='429：限流冷却' WHERE id=?", [self.key.id])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn success(&self, prompt: i64, completion: i64, model: &str) -> Result<()> {
        let mut conn = self.pool.store.lock();
        let tx = conn.transaction()?;
        if !self.credential_is_current(&tx)? || self.model != model {
            return Ok(());
        }
        // A newer model failure fences recovery, not usage already consumed.
        tx.execute("UPDATE channel_keys SET successes=successes+1,prompt_tokens=prompt_tokens+?,completion_tokens=completion_tokens+?,last_success_at=? WHERE id=? AND generation=?",params![prompt.max(0),completion.max(0),Utc::now().to_rfc3339(),self.key.id,self.key.generation])?;
        if self.model_is_current(&tx, model)? {
            tx.execute("UPDATE channel_keys SET last_error='',cooldown_until=NULL,rate_limit_count=0 WHERE id=? AND generation=?",params![self.key.id,self.key.generation])?;
            // Keep the epoch so pre-cooldown replies cannot recover it later.
            tx.execute(
                "UPDATE channel_key_models SET unavailable_until='',rate_limit_count=0 WHERE key_id=? AND model=? AND generation=?",
                params![self.key.id, model, self.model_generation],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}

impl Drop for KeyLease {
    fn drop(&mut self) {
        if self.recovery || self.model_recovery {
            let mut state = self.pool.state.lock().unwrap_or_else(|p| p.into_inner());
            if self.recovery {
                state.recovery.remove(&(self.key.id, self.key.generation));
            }
            if self.model_recovery {
                state.model_recovery.remove(&(
                    self.key.id,
                    self.key.generation,
                    self.model.clone(),
                    self.model_generation,
                ));
            }
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
    fn late_success_counts_usage_without_recovering_a_newer_failure() {
        let (pool, id) = setup();
        let key_id = pool.store.list_keys(id).unwrap()[0].id;
        let success = pool
            .acquire(id, "m", &HashSet::new(), Some(key_id))
            .unwrap()
            .unwrap();
        let limited = pool
            .acquire(id, "m", &HashSet::new(), Some(key_id))
            .unwrap()
            .unwrap();
        limited
            .failure(&crate::resilience::classify_status(429), "m")
            .unwrap();
        success.success(100, 20, "m").unwrap();
        let key = pool.store.list_keys(id).unwrap().remove(0);
        assert_eq!(
            (
                key.attempts,
                key.successes,
                key.prompt_tokens,
                key.completion_tokens
            ),
            (2, 1, 100, 20)
        );
        assert_eq!(key.rate_limits, 1);
        assert!(key.last_error.contains("429"));
        assert!(pool.retry_after_model(id, "m") >= Duration::from_secs(290));
        assert!(pool
            .acquire(id, "m", &HashSet::new(), Some(key_id))
            .unwrap()
            .is_none());
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

    #[test]
    fn concurrent_models_keep_independent_cooldowns_and_recovery_epochs() {
        let (pool, id) = setup();
        let key = pool.store.list_keys(id).unwrap()[0].id;
        let x = pool
            .acquire(id, "x", &HashSet::new(), Some(key))
            .unwrap()
            .unwrap();
        let y = pool
            .acquire(id, "y", &HashSet::new(), Some(key))
            .unwrap()
            .unwrap();
        let stale = pool.acquire_for_probe(id, "x", Some(key)).unwrap().unwrap();
        x.failure(&FailureInfo::new(FailureClass::RateLimit), "x")
            .unwrap();
        y.failure(&FailureInfo::new(FailureClass::RateLimit), "y")
            .unwrap();
        assert!(pool.retry_after_model(id, "x") >= Duration::from_secs(290));
        assert!(pool.retry_after_model(id, "y") >= Duration::from_secs(290));
        assert_eq!(pool.store.list_keys(id).unwrap()[0].rate_limits, 2);
        stale.success(0, 0, "x").unwrap();
        assert!(pool
            .acquire(id, "x", &HashSet::new(), Some(key))
            .unwrap()
            .is_none());
        pool.acquire_for_probe(id, "x", Some(key))
            .unwrap()
            .unwrap()
            .success(1, 2, "x")
            .unwrap();
        assert!(pool
            .acquire(id, "x", &HashSet::new(), Some(key))
            .unwrap()
            .is_some());
        assert!(pool
            .acquire(id, "y", &HashSet::new(), Some(key))
            .unwrap()
            .is_none());
        // Clearing a cooldown must not resurrect the old epoch.
        stale
            .failure(&FailureInfo::new(FailureClass::RateLimit), "x")
            .unwrap();
        assert_eq!(pool.retry_after_model(id, "x"), Duration::ZERO);
    }

    #[test]
    fn model_failure_does_not_discard_a_concurrent_credential_auth_failure() {
        let (pool, id) = setup();
        let key = pool.store.list_keys(id).unwrap()[0].id;
        let x = pool
            .acquire(id, "x", &HashSet::new(), Some(key))
            .unwrap()
            .unwrap();
        let y = pool
            .acquire(id, "y", &HashSet::new(), Some(key))
            .unwrap()
            .unwrap();
        x.failure(&FailureInfo::new(FailureClass::RateLimit), "x")
            .unwrap();
        y.failure(
            &FailureInfo {
                class: Some(FailureClass::Auth),
                status_code: 401,
                retry_after: Duration::ZERO,
            },
            "y",
        )
        .unwrap();
        assert!(pool.store.list_keys(id).unwrap()[0].auth_invalid);
        x.success(0, 0, "x").unwrap();
        assert!(pool
            .acquire(id, "other", &HashSet::new(), Some(key))
            .unwrap()
            .is_none());
    }

    #[test]
    fn replaced_credential_rejects_all_older_model_results() {
        let (pool, id) = setup();
        let key = pool.store.list_keys(id).unwrap()[0].id;
        let lease = pool
            .acquire(id, "x", &HashSet::new(), Some(key))
            .unwrap()
            .unwrap();
        pool.store
            .patch_key(
                id,
                key,
                &crate::store_keys::KeyPatch {
                    secret: Some("replacement-key".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        lease
            .failure(&FailureInfo::new(FailureClass::RateLimit), "x")
            .unwrap();
        lease.success(10, 20, "x").unwrap();
        assert_eq!(pool.retry_after_model(id, "x"), Duration::ZERO);
        assert_eq!(pool.store.list_keys(id).unwrap()[0].successes, 0);
    }

    #[test]
    fn disabling_and_reenabling_a_key_invalidates_in_flight_leases() {
        let (pool, id) = setup();
        let key_id = pool.store.list_keys(id).unwrap()[0].id;
        let lease = pool
            .acquire(id, "x", &HashSet::new(), Some(key_id))
            .unwrap()
            .unwrap();

        pool.store
            .patch_key(
                id,
                key_id,
                &crate::store_keys::KeyPatch {
                    enabled: Some(false),
                    ..Default::default()
                },
            )
            .unwrap();
        pool.store
            .patch_key(
                id,
                key_id,
                &crate::store_keys::KeyPatch {
                    enabled: Some(true),
                    ..Default::default()
                },
            )
            .unwrap();

        lease
            .failure(&FailureInfo::new(FailureClass::RateLimit), "x")
            .unwrap();
        lease.success(10, 20, "x").unwrap();

        let key = pool
            .store
            .list_keys(id)
            .unwrap()
            .into_iter()
            .find(|key| key.id == key_id)
            .unwrap();
        assert!(key.enabled);
        assert_eq!(
            (key.successes, key.prompt_tokens, key.completion_tokens),
            (0, 0, 0)
        );
        assert!(pool
            .store
            .model_cooling_key_ids(id, "x")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn model_recovery_is_exclusive_independent_and_released_on_cancel() {
        let (pool, id) = setup();
        let key = pool.store.list_keys(id).unwrap().remove(0);
        let lease = pool
            .acquire(id, "x", &HashSet::new(), Some(key.id))
            .unwrap()
            .unwrap();
        lease
            .failure(&FailureInfo::new(FailureClass::RateLimit), "x")
            .unwrap();
        drop(lease);
        pool.store
            .lock()
            .execute(
                "UPDATE channel_key_models SET unavailable_until=? WHERE key_id=? AND model='x'",
                params![
                    (Utc::now() - chrono::Duration::seconds(1)).to_rfc3339(),
                    key.id
                ],
            )
            .unwrap();
        let recovery = pool
            .acquire(id, "x", &HashSet::new(), Some(key.id))
            .unwrap()
            .unwrap();
        assert!(pool
            .acquire(id, "x", &HashSet::new(), Some(key.id))
            .unwrap()
            .is_none());
        let other = pool
            .acquire(id, "y", &HashSet::new(), Some(key.id))
            .unwrap()
            .unwrap();
        other.success(1, 2, "y").unwrap();
        assert!(pool
            .acquire(id, "x", &HashSet::new(), Some(key.id))
            .unwrap()
            .is_none());
        drop(recovery);
        let retry = pool
            .acquire(id, "x", &HashSet::new(), Some(key.id))
            .unwrap()
            .unwrap();
        retry.success(1, 2, "x").unwrap();
        drop(retry);
        let a = pool
            .acquire(id, "x", &HashSet::new(), Some(key.id))
            .unwrap()
            .unwrap();
        let b = pool
            .acquire(id, "x", &HashSet::new(), Some(key.id))
            .unwrap()
            .unwrap();
        assert_eq!(a.key.id, b.key.id);
    }
}
