//! Persists desktop channels, request logs and settings in a single local
//! SQLite file. Ported from the Go `internal/channel/store.go`.
//!
//! The database is guarded by one connection (the Go version capped its pool
//! at 1): desktop workloads are small and serialized writes keep probe updates
//! and log writes deterministic.

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};

use chrono::{DateTime, Local, TimeZone, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// The Go sentinel for a missing channel; handlers map it to 404.
pub const ERR_CHANNEL_NOT_FOUND: &str = "desktop channel not found";

/// Errors surfaced by the store.
#[derive(Debug)]
pub enum StoreError {
    Invalid(String),
    NotFound(i64),
    Db(rusqlite::Error),
    Json(serde_json::Error),
    Closed,
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Invalid(message) => write!(f, "{message}"),
            StoreError::NotFound(id) => write!(f, "desktop channel {id}: {ERR_CHANNEL_NOT_FOUND}"),
            StoreError::Db(err) => write!(f, "{err}"),
            StoreError::Json(err) => write!(f, "{err}"),
            StoreError::Closed => write!(f, "desktop channel store is closed"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(err: rusqlite::Error) -> Self {
        StoreError::Db(err)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(err: serde_json::Error) -> Self {
        StoreError::Json(err)
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// Channel is the local-only channel record used by the desktop gateway. It
/// deliberately has no user, group, billing, or cloud-sync fields.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Channel {
    pub id: i64,
    pub name: String,
    pub provider: String,
    pub api_mode: String,
    pub endpoint: String,
    pub api_key: String,
    pub primary_model: String,
    pub extra_models: Vec<String>,
    pub enabled: bool,
    #[serde(skip)]
    pub interval_seconds: i64,
    pub last_status: String,
    pub last_latency_ms: Option<i64>,
    pub last_ttft_ms: Option<i64>,
    pub last_ping_latency_ms: Option<i64>,
    pub last_error: String,
    pub last_checked_at: Option<DateTime<Utc>>,
    #[serde(skip)]
    pub probe_streak: i64,
    pub failure_count: i64,
    pub circuit_state: String,
    pub cooldown_until: Option<DateTime<Utc>>,
    pub injection_state: String,
    pub injection_tokens: Option<i64>,
    pub injection_content: String,
    pub injection_checked_at: Option<DateTime<Utc>>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(skip)]
    pub effective_interval_seconds: i64,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub model_health: Vec<ModelHealth>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub model_performance: Vec<ModelPerformance>,
}

pub const MODEL_PERFORMANCE_MAX_AGE_SECS: i64 = 30 * 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPerformance {
    pub model: String,
    pub ttft_ms: f64,
    pub last_ttft_ms: i64,
    pub samples: i64,
    pub updated_at: DateTime<Utc>,
}

impl Channel {
    /// Returns every configured model ID, including the legacy primary-model
    /// column when reading older databases. Model IDs remain distinct; only
    /// exact duplicates are removed.
    pub fn configured_models(&self) -> Vec<String> {
        let mut models = Vec::with_capacity(self.extra_models.len() + 1);
        let mut seen = HashSet::new();
        for model in std::iter::once(&self.primary_model).chain(&self.extra_models) {
            let model = model.trim();
            if !model.is_empty() && seen.insert(model.to_string()) {
                models.push(model.to_string());
            }
        }
        models
    }

    pub fn first_model(&self) -> Option<String> {
        self.configured_models().into_iter().next()
    }

    pub(crate) fn same_upstream(&self, other: &Channel) -> bool {
        (self.endpoint == other.endpoint
            || crate::probe::same_endpoint(&self.endpoint, &other.endpoint))
            && self.provider == other.provider
            && self.api_mode == other.api_mode
    }

    pub(crate) fn has_model(&self, model: &str) -> bool {
        !model.trim().is_empty()
            && self
                .configured_models()
                .iter()
                .any(|configured| configured.eq_ignore_ascii_case(model.trim()))
    }

    fn reset_upstream_runtime(&mut self) {
        self.reset_primary_runtime();
        self.failure_count = 0;
        self.circuit_state = "closed".into();
        self.cooldown_until = None;
        self.model_health.clear();
        self.model_performance.clear();
    }

    fn reset_primary_runtime(&mut self) {
        self.last_status = "unknown".into();
        self.last_latency_ms = None;
        self.last_ttft_ms = None;
        self.last_ping_latency_ms = None;
        self.last_error.clear();
        self.last_checked_at = None;
        self.probe_streak = 0;
        self.injection_state = "unknown".into();
        self.injection_tokens = None;
        self.injection_content.clear();
        self.injection_checked_at = None;
    }

    pub fn fresh_model_performance(&self, model: &str) -> Option<&ModelPerformance> {
        let now = Utc::now();
        self.model_performance.iter().find(|p| {
            let age = now.signed_duration_since(p.updated_at).num_seconds();
            p.model.eq_ignore_ascii_case(model)
                && (0..MODEL_PERFORMANCE_MAX_AGE_SECS).contains(&age)
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelHealth {
    pub model: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttft_ms: Option<i64>,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<DateTime<Utc>>,
}

/// A real-time event for desktop request logs.
#[derive(Debug, Clone, Serialize)]
pub struct LogEvent {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log: Option<RequestLog>,
}

/// An audit of a single proxied request through HeiGate.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RequestLog {
    pub id: i64,
    pub model: String,
    pub channel_name: String,
    pub status_code: i64,
    pub latency_ms: i64,
    pub ttft_ms: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub total_tokens: i64,
    pub is_stream: bool,
    pub error_message: String,
    pub failover_from: String,
    pub is_failover: bool,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub upstream_model: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub endpoint: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub client_ip: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub user_agent: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub request_path: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub finish_reason: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub trace_json: String,
    pub created_at: DateTime<Utc>,
}

const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS desktop_channels (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT NOT NULL,
  provider TEXT NOT NULL,
  api_mode TEXT NOT NULL DEFAULT 'chat_completions',
  endpoint TEXT NOT NULL,
  api_key TEXT NOT NULL,
  primary_model TEXT NOT NULL,
  extra_models_json TEXT NOT NULL DEFAULT '[]',
  enabled INTEGER NOT NULL DEFAULT 1,
  interval_seconds INTEGER NOT NULL DEFAULT 300,
  last_status TEXT NOT NULL DEFAULT 'unknown',
  last_latency_ms INTEGER,
  last_ttft_ms INTEGER,
  last_ping_latency_ms INTEGER,
  last_error TEXT NOT NULL DEFAULT '',
  last_checked_at TEXT,
  probe_streak INTEGER NOT NULL DEFAULT 0,
  failure_count INTEGER NOT NULL DEFAULT 0,
  circuit_state TEXT NOT NULL DEFAULT 'closed',
  cooldown_until TEXT,
  injection_state TEXT NOT NULL DEFAULT 'unknown',
  injection_tokens INTEGER,
  injection_content TEXT NOT NULL DEFAULT '',
  injection_checked_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_desktop_channels_enabled ON desktop_channels(enabled);

CREATE TABLE IF NOT EXISTS desktop_request_logs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  model TEXT NOT NULL,
  channel_name TEXT NOT NULL,
  status_code INTEGER NOT NULL,
  latency_ms INTEGER NOT NULL,
  ttft_ms INTEGER NOT NULL DEFAULT 0,
  prompt_tokens INTEGER NOT NULL DEFAULT 0,
  completion_tokens INTEGER NOT NULL DEFAULT 0,
  total_tokens INTEGER NOT NULL DEFAULT 0,
  is_stream INTEGER NOT NULL DEFAULT 0,
  error_message TEXT NOT NULL DEFAULT '',
  failover_from TEXT NOT NULL DEFAULT '',
  is_failover INTEGER NOT NULL DEFAULT 0,
  upstream_model TEXT NOT NULL DEFAULT '',
  endpoint TEXT NOT NULL DEFAULT '',
  client_ip TEXT NOT NULL DEFAULT '',
  user_agent TEXT NOT NULL DEFAULT '',
  request_path TEXT NOT NULL DEFAULT '',
  finish_reason TEXT NOT NULL DEFAULT '',
  trace_json TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_desktop_request_logs_created_at ON desktop_request_logs(created_at);

CREATE TABLE IF NOT EXISTS desktop_settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS desktop_model_health (
  channel_id INTEGER NOT NULL,
  model TEXT NOT NULL,
  status TEXT NOT NULL,
  latency_ms INTEGER,
  ttft_ms INTEGER,
  error TEXT NOT NULL DEFAULT '',
  checked_at TEXT,
  PRIMARY KEY (channel_id, model),
  FOREIGN KEY (channel_id) REFERENCES desktop_channels(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS desktop_model_performance (
  channel_id INTEGER NOT NULL,
  model TEXT NOT NULL,
  ttft_ms REAL NOT NULL,
  last_ttft_ms INTEGER NOT NULL,
  samples INTEGER NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (channel_id, model),
  FOREIGN KEY (channel_id) REFERENCES desktop_channels(id) ON DELETE CASCADE
);
"#;

/// Migrations applied to databases created before the corresponding columns
/// existed. "duplicate column" errors are tolerated, matching the Go code.
const MIGRATIONS: &[&str] = &[
    "ALTER TABLE desktop_channels ADD COLUMN failure_count INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE desktop_channels ADD COLUMN circuit_state TEXT NOT NULL DEFAULT 'closed'",
    "ALTER TABLE desktop_channels ADD COLUMN cooldown_until TEXT",
    "ALTER TABLE desktop_channels ADD COLUMN last_ttft_ms INTEGER",
    "ALTER TABLE desktop_channels ADD COLUMN injection_state TEXT NOT NULL DEFAULT 'unknown'",
    "ALTER TABLE desktop_channels ADD COLUMN injection_tokens INTEGER",
    "ALTER TABLE desktop_channels ADD COLUMN injection_content TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE desktop_channels ADD COLUMN injection_checked_at TEXT",
    "ALTER TABLE desktop_channels ADD COLUMN probe_streak INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE desktop_request_logs ADD COLUMN ttft_ms INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE desktop_request_logs ADD COLUMN prompt_tokens INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE desktop_request_logs ADD COLUMN completion_tokens INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE desktop_request_logs ADD COLUMN total_tokens INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE desktop_request_logs ADD COLUMN is_stream INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE desktop_request_logs ADD COLUMN upstream_model TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE desktop_request_logs ADD COLUMN endpoint TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE desktop_request_logs ADD COLUMN client_ip TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE desktop_request_logs ADD COLUMN user_agent TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE desktop_request_logs ADD COLUMN request_path TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE desktop_request_logs ADD COLUMN finish_reason TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE desktop_request_logs ADD COLUMN trace_json TEXT NOT NULL DEFAULT ''",
];

const CHANNEL_COLUMNS: &str = "id, name, provider, api_mode, endpoint, api_key, primary_model, \
extra_models_json, enabled, interval_seconds, last_status, \
last_latency_ms, last_ttft_ms, last_ping_latency_ms, last_error, last_checked_at, \
probe_streak, failure_count, circuit_state, cooldown_until, \
injection_state, injection_tokens, injection_content, injection_checked_at, \
created_at, updated_at";

pub struct Store {
    conn: Mutex<Connection>,
    pub(crate) runtime_mu: Mutex<()>,
    subscribers: RwLock<Vec<(u64, mpsc::Sender<LogEvent>)>>,
    next_sub_id: Mutex<u64>,
}

impl Store {
    /// Opens or creates a local SQLite database at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Arc<Self>> {
        let path = path.as_ref();
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        crate::store_keys::backup_before_migration(&conn, path)?;
        Self::from_connection(conn)
    }

    /// Opens an in-memory database (tests).
    pub fn open_in_memory() -> Result<Arc<Self>> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        Self::from_connection(conn)
    }

    /// Wraps an already-opened connection.
    pub fn from_connection(conn: Connection) -> Result<Arc<Self>> {
        let store = Arc::new(Store {
            conn: Mutex::new(conn),
            runtime_mu: Mutex::new(()),
            subscribers: RwLock::new(Vec::new()),
            next_sub_id: Mutex::new(1),
        });
        store.initialize()?;
        Ok(store)
    }

    fn initialize(&self) -> Result<()> {
        let conn = self.lock();
        conn.execute_batch("PRAGMA foreign_keys = ON")?;
        conn.execute_batch(SCHEMA_SQL)?;
        // Remove the obsolete per-client model mapping setting.
        conn.execute(
            "DELETE FROM desktop_settings WHERE key = 'claude_desktop_model_mappings'",
            [],
        )?;
        conn.execute(
            "DELETE FROM desktop_settings WHERE key IN (
                'client_config_codex',
                'client_config_claude_desktop',
                'client_config_claude_code',
                'client_config_generic'
            )",
            [],
        )?;
        for statement in MIGRATIONS {
            if let Err(err) = conn.execute_batch(statement) {
                let message = err.to_string().to_lowercase();
                if !message.contains("duplicate column") {
                    return Err(StoreError::Db(err));
                }
            }
        }
        // Channel priority was never exposed in the UI and no routing strategy
        // read it; drop it where the SQLite build supports DROP COLUMN.
        let _ = conn.execute_batch("ALTER TABLE desktop_channels DROP COLUMN priority");
        // Auto-repair legacy false-positive 502 logs where streaming completed
        // tokens but was flagged context canceled.
        let _ = conn.execute_batch(
            "UPDATE desktop_request_logs
             SET status_code = 200,
                 finish_reason = 'stop',
                 error_message = '流式生成完成 (客户端在接收后断开连接)'
             WHERE status_code = 502
               AND completion_tokens > 0
               AND error_message LIKE '%context canceled%'",
        );
        drop(conn);
        self.prune_all_model_health()?;
        self.initialize_keys()?;
        Ok(())
    }

    pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn prune_all_model_health(&self) -> Result<()> {
        let channels: Vec<(i64, String, Option<String>)> = {
            let conn = self.lock();
            let mut stmt =
                conn.prepare("SELECT id, primary_model, extra_models_json FROM desktop_channels")?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (id, primary, extra_json) in channels {
            let mut models = vec![primary];
            models.extend(decode_extra_models(extra_json.as_deref()));
            self.prune_model_health(id, &models)?;
        }
        Ok(())
    }

    // ---- channels -------------------------------------------------------

    pub fn list(&self) -> Result<Vec<Channel>> {
        let mut channels: Vec<Channel> = {
            let conn = self.lock();
            let sql = format!("SELECT {CHANNEL_COLUMNS} FROM desktop_channels ORDER BY id ASC");
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([], scan_channel_row)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for channel in &mut channels {
            self.load_model_health(channel)?;
        }
        Ok(channels)
    }

    pub fn get(&self, id: i64) -> Result<Channel> {
        let mut channel = {
            let conn = self.lock();
            let sql = format!("SELECT {CHANNEL_COLUMNS} FROM desktop_channels WHERE id = ?");
            let found = conn
                .query_row(&sql, params![id], scan_channel_row)
                .optional()?;
            found.ok_or(StoreError::NotFound(id))?
        };
        self.load_model_health(&mut channel)?;
        Ok(channel)
    }

    fn load_model_health(&self, channel: &mut Channel) -> Result<()> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT model, status, latency_ms, ttft_ms, error, checked_at \
             FROM desktop_model_health WHERE channel_id = ? ORDER BY model",
        )?;
        let rows = stmt.query_map(params![channel.id], |row| {
            let checked: Option<String> = row.get(5)?;
            Ok(ModelHealth {
                model: row.get(0)?,
                status: row.get(1)?,
                latency_ms: row.get(2)?,
                ttft_ms: row.get(3)?,
                error: row.get(4)?,
                checked_at: parse_time(checked.as_deref()),
            })
        })?;
        channel.model_health = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        let mut stmt = conn.prepare("SELECT model, ttft_ms, last_ttft_ms, samples, updated_at FROM desktop_model_performance WHERE channel_id=? ORDER BY model")?;
        let rows = stmt.query_map([channel.id], |row| {
            let updated: String = row.get(4)?;
            Ok(ModelPerformance {
                model: row.get(0)?,
                ttft_ms: row.get(1)?,
                last_ttft_ms: row.get(2)?,
                samples: row.get(3)?,
                updated_at: parse_time(Some(&updated)).unwrap_or_default(),
            })
        })?;
        channel.model_performance = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(())
    }

    pub(crate) fn record_live_model_ttft(
        &self,
        channel: &Channel,
        model: &str,
        ttft_ms: i64,
        observed_at: DateTime<Utc>,
        lease: &crate::key_pool::KeyLease,
        started_at: DateTime<Utc>,
    ) -> Result<()> {
        self.record_model_ttft_checked(
            channel,
            model,
            ttft_ms,
            observed_at,
            Some((lease, started_at)),
        )
    }

    #[cfg(test)]
    pub(crate) fn record_model_ttft(
        &self,
        channel: &Channel,
        model: &str,
        ttft_ms: i64,
        observed_at: DateTime<Utc>,
    ) -> Result<()> {
        self.record_model_ttft_checked(channel, model, ttft_ms, observed_at, None)
    }

    fn record_model_ttft_checked(
        &self,
        channel: &Channel,
        model: &str,
        ttft_ms: i64,
        observed_at: DateTime<Utc>,
        expected: Option<(&crate::key_pool::KeyLease, DateTime<Utc>)>,
    ) -> Result<()> {
        if !(1..=600_000).contains(&ttft_ms) {
            return Ok(());
        }
        let _runtime_guard = self.runtime_mu.lock().unwrap_or_else(|p| p.into_inner());
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let sql = format!("SELECT {CHANNEL_COLUMNS} FROM desktop_channels WHERE id=?");
        let Some(current) = tx
            .query_row(&sql, [channel.id], scan_channel_row)
            .optional()?
        else {
            return Ok(());
        };
        if !current.same_upstream(channel) || !current.has_model(model) {
            return Ok(());
        }
        if let Some((lease, started_at)) = expected {
            if !current.enabled || !lease.is_current_in(&tx, model)? {
                return Ok(());
            }
            let checked: Option<String> = tx
                .query_row(
                    "SELECT checked_at FROM desktop_model_health WHERE channel_id=? AND model=?",
                    params![channel.id, model],
                    |row| row.get(0),
                )
                .optional()?
                .flatten();
            if parse_time(checked.as_deref()).is_some_and(|time| time > started_at) {
                return Ok(());
            }
        }
        let previous: Option<(f64, i64, String)> = tx.query_row(
            "SELECT ttft_ms, samples, updated_at FROM desktop_model_performance WHERE channel_id=? AND model=?",
            params![channel.id, model], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))
        ).optional()?;
        if previous.as_ref().is_some_and(|(_, _, updated)| {
            parse_time(Some(updated)).is_some_and(|time| time > observed_at)
        }) {
            return Ok(());
        }
        let (average, samples, updated) = previous
            .map(|(average, samples, updated)| {
                let updated = parse_time(Some(&updated)).unwrap_or_default();
                if observed_at.signed_duration_since(updated).num_seconds()
                    >= MODEL_PERFORMANCE_MAX_AGE_SECS
                {
                    (ttft_ms as f64, 1, observed_at)
                } else {
                    (
                        average * 0.8 + ttft_ms as f64 * 0.2,
                        samples + 1,
                        observed_at.max(updated),
                    )
                }
            })
            .unwrap_or((ttft_ms as f64, 1, observed_at));
        tx.execute("INSERT INTO desktop_model_performance(channel_id,model,ttft_ms,last_ttft_ms,samples,updated_at) VALUES(?,?,?,?,?,?) \
            ON CONFLICT(channel_id,model) DO UPDATE SET ttft_ms=excluded.ttft_ms,last_ttft_ms=excluded.last_ttft_ms,samples=excluded.samples,updated_at=excluded.updated_at",
            params![channel.id,model,average,ttft_ms,samples,format_time(updated)])?;
        tx.commit()?;
        Ok(())
    }

    pub fn set_model_health(&self, channel_id: i64, health: &ModelHealth) -> Result<()> {
        let _runtime_guard = self.runtime_mu.lock().unwrap_or_else(|p| p.into_inner());
        let conn = self.lock();
        conn.execute(
            "INSERT INTO desktop_model_health(channel_id, model, status, latency_ms, ttft_ms, error, checked_at) \
             VALUES(?,?,?,?,?,?,?) \
             ON CONFLICT(channel_id, model) DO UPDATE SET status=excluded.status, latency_ms=excluded.latency_ms, \
             ttft_ms=excluded.ttft_ms, error=excluded.error, checked_at=excluded.checked_at",
            params![
                channel_id,
                health.model,
                health.status,
                health.latency_ms,
                health.ttft_ms,
                health.error,
                health.checked_at.map(format_time),
            ],
        )?;
        Ok(())
    }

    /// Whether a reply still belongs to the channel's configured upstream and model.
    pub fn channel_config_is_current(&self, channel: &Channel, model: &str) -> Result<bool> {
        let conn = self.lock();
        let sql = format!("SELECT {CHANNEL_COLUMNS} FROM desktop_channels WHERE id=?");
        Ok(conn
            .query_row(&sql, [channel.id], scan_channel_row)
            .optional()?
            .is_some_and(|current| {
                current.enabled
                    && current.same_upstream(channel)
                    && (model.is_empty() || current.has_model(model))
            }))
    }

    /// Live requests update health without replacing independent probe timings.
    pub fn record_model_outcome(
        &self,
        channel: &Channel,
        model: &str,
        succeeded: bool,
        error: &str,
        started_at: DateTime<Utc>,
    ) -> Result<bool> {
        let _guard = self.runtime_mu.lock().unwrap_or_else(|p| p.into_inner());
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let sql = format!("SELECT {CHANNEL_COLUMNS} FROM desktop_channels WHERE id=?");
        let Some(current) = tx
            .query_row(&sql, [channel.id], scan_channel_row)
            .optional()?
        else {
            return Ok(false);
        };
        if !current.enabled
            || current.endpoint != channel.endpoint
            || current.provider != channel.provider
            || current.api_mode != channel.api_mode
            || !current.has_model(model)
        {
            return Ok(false);
        }
        let checked_at: Option<String> = tx
            .query_row(
                "SELECT checked_at FROM desktop_model_health WHERE channel_id=? AND model=?",
                params![channel.id, model],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        if parse_time(checked_at.as_deref()).is_some_and(|checked| checked > started_at) {
            return Ok(false);
        }
        let status = if succeeded {
            crate::probe::STATUS_OPERATIONAL
        } else {
            crate::probe::STATUS_ERROR
        };
        tx.execute(
            "INSERT INTO desktop_model_health(channel_id,model,status,error,checked_at) VALUES(?,?,?,?,?) \
             ON CONFLICT(channel_id,model) DO UPDATE SET status=excluded.status,error=excluded.error,checked_at=excluded.checked_at",
            params![channel.id, model, status, if succeeded { "" } else { error }, format_time(started_at)],
        )?;
        tx.commit()?;
        Ok(true)
    }

    pub fn save(&self, channel: &mut Channel) -> Result<()> {
        let _runtime_guard = self.runtime_mu.lock().unwrap_or_else(|p| p.into_inner());
        self.save_locked(channel)
    }

    fn save_locked(&self, channel: &mut Channel) -> Result<()> {
        let primary = channel.primary_model.trim().to_string();
        channel.primary_model = primary.clone();
        let mut seen = HashSet::new();
        let mut extra_models_normalized = Vec::with_capacity(channel.extra_models.len());
        for model in std::mem::take(&mut channel.extra_models) {
            let trimmed = model.trim();
            if !trimmed.is_empty() && trimmed != primary && seen.insert(trimmed.to_string()) {
                extra_models_normalized.push(trimmed.to_string());
            }
        }
        channel.extra_models = extra_models_normalized;
        let extra_models = serde_json::to_string(&channel.extra_models)?;
        let now = Utc::now();
        if channel.created_at.is_none() {
            channel.created_at = Some(now);
        }
        channel.updated_at = Some(now);
        if channel.api_mode.is_empty() {
            channel.api_mode = "chat_completions".to_string();
        }
        if channel.interval_seconds < 0 {
            channel.interval_seconds = 0;
        }
        if channel.last_status.is_empty() {
            channel.last_status = "unknown".to_string();
        }
        if channel.circuit_state.is_empty() {
            channel.circuit_state = "closed".to_string();
        }
        channel.effective_interval_seconds = effective_interval_seconds();

        let created_at = format_time(channel.created_at.unwrap_or(now));
        let updated_at = format_time(channel.updated_at.unwrap_or(now));

        {
            let mut conn = self.lock();
            let conn = conn.transaction()?;
            if channel.id == 0 {
                conn.execute(
                    "INSERT INTO desktop_channels
                     (name, provider, api_mode, endpoint, api_key, primary_model, extra_models_json,
                      enabled, interval_seconds, last_status, last_latency_ms, last_ttft_ms,
                      last_ping_latency_ms, last_error, last_checked_at, probe_streak, failure_count, circuit_state,
                      cooldown_until, injection_state, injection_tokens, injection_content, injection_checked_at,
                      created_at, updated_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    params![
                        channel.name,
                        channel.provider,
                        channel.api_mode,
                        channel.endpoint,
                        channel.api_key,
                        channel.primary_model,
                        extra_models,
                        channel.enabled as i64,
                        channel.interval_seconds,
                        channel.last_status,
                        channel.last_latency_ms,
                        channel.last_ttft_ms,
                        channel.last_ping_latency_ms,
                        channel.last_error,
                        channel.last_checked_at.map(format_time),
                        channel.probe_streak,
                        channel.failure_count,
                        channel.circuit_state,
                        channel.cooldown_until.map(format_time),
                        channel.injection_state,
                        channel.injection_tokens,
                        channel.injection_content,
                        channel.injection_checked_at.map(format_time),
                        created_at,
                        updated_at,
                    ],
                )?;
                channel.id = conn.last_insert_rowid();
            } else {
                let sql = format!("SELECT {CHANNEL_COLUMNS} FROM desktop_channels WHERE id=?");
                let previous = conn.query_row(&sql, [channel.id], scan_channel_row)?;
                if !previous.same_upstream(channel) {
                    channel.reset_upstream_runtime();
                    conn.execute(
                        "DELETE FROM desktop_model_performance WHERE channel_id=?",
                        [channel.id],
                    )?;
                    conn.execute(
                        "DELETE FROM desktop_model_health WHERE channel_id=?",
                        [channel.id],
                    )?;
                    conn.execute("UPDATE channel_keys SET auth_invalid=0,rate_limit_count=0,cooldown_until=NULL,last_error='',generation=generation+1 WHERE channel_id=?", [channel.id])?;
                    conn.execute("DELETE FROM channel_key_models WHERE key_id IN (SELECT id FROM channel_keys WHERE channel_id=?)", [channel.id])?;
                } else {
                    for model in
                        std::iter::once(&previous.primary_model).chain(&previous.extra_models)
                    {
                        if model.trim().is_empty() {
                            continue;
                        }
                        if !channel.has_model(model) {
                            // Retain the epoch when removing a model, fencing old replies if it is added again.
                            conn.execute("UPDATE channel_key_models SET unavailable_until='',rate_limit_count=0,generation=generation+1 WHERE key_id IN (SELECT id FROM channel_keys WHERE channel_id=?) AND model=?", params![channel.id, model])?;
                            conn.execute("INSERT OR IGNORE INTO channel_key_models(key_id,model,unavailable_until,generation) SELECT id,?,'',1 FROM channel_keys WHERE channel_id=?", params![model, channel.id])?;
                        }
                    }
                }
                conn.execute(
                    "UPDATE desktop_channels SET
                     name=?, provider=?, api_mode=?, endpoint=?, api_key=?, primary_model=?, extra_models_json=?,
                     enabled=?, interval_seconds=?, last_status=?, last_latency_ms=?, last_ttft_ms=?,
                     last_ping_latency_ms=?, last_error=?, last_checked_at=?, probe_streak=?, failure_count=?, circuit_state=?,
                     cooldown_until=?, injection_state=?, injection_tokens=?, injection_content=?, injection_checked_at=?,
                     created_at=?, updated_at=?
                     WHERE id=?",
                    params![
                        channel.name,
                        channel.provider,
                        channel.api_mode,
                        channel.endpoint,
                        channel.api_key,
                        channel.primary_model,
                        extra_models,
                        channel.enabled as i64,
                        channel.interval_seconds,
                        channel.last_status,
                        channel.last_latency_ms,
                        channel.last_ttft_ms,
                        channel.last_ping_latency_ms,
                        channel.last_error,
                        channel.last_checked_at.map(format_time),
                        channel.probe_streak,
                        channel.failure_count,
                        channel.circuit_state,
                        channel.cooldown_until.map(format_time),
                        channel.injection_state,
                        channel.injection_tokens,
                        channel.injection_content,
                        channel.injection_checked_at.map(format_time),
                        created_at,
                        updated_at,
                        channel.id,
                    ],
                )?;
            }
            crate::store_keys::sync_legacy_key(&conn, channel.id, &channel.api_key)?;
            conn.commit()?;
        }

        let models = channel.configured_models();
        self.prune_model_health(channel.id, &models)
    }

    /// Removes snapshots for models no longer configured on the channel, so
    /// deleted models do not reappear as stale health rows.
    pub fn prune_model_health(&self, channel_id: i64, models: &[String]) -> Result<()> {
        if channel_id <= 0 {
            return Ok(());
        }
        let mut kept: Vec<String> = Vec::new();
        for model in models {
            let model = model.trim();
            if model.is_empty() || kept.iter().any(|m| m == model) {
                continue;
            }
            kept.push(model.to_string());
        }
        let conn = self.lock();
        if kept.is_empty() {
            conn.execute(
                "DELETE FROM desktop_model_health WHERE channel_id = ?",
                params![channel_id],
            )?;
            conn.execute(
                "DELETE FROM desktop_model_performance WHERE channel_id=?",
                [channel_id],
            )?;
            return Ok(());
        }
        let placeholders = vec!["?"; kept.len()].join(",");
        let sql = format!(
            "DELETE FROM desktop_model_health WHERE channel_id = ? AND model NOT IN ({placeholders})"
        );
        let mut values: Vec<Box<dyn rusqlite::ToSql>> = Vec::with_capacity(kept.len() + 1);
        values.push(Box::new(channel_id));
        for model in kept {
            values.push(Box::new(model));
        }
        let refs: Vec<&dyn rusqlite::ToSql> = values.iter().map(|v| v.as_ref()).collect();
        conn.execute(&sql, refs.as_slice())?;
        let sql = format!("DELETE FROM desktop_model_performance WHERE channel_id=? AND model NOT IN ({placeholders})");
        conn.execute(&sql, refs.as_slice())?;
        Ok(())
    }

    /// Re-reads the channel by id, lets `mutate` adjust it, and saves. Callers
    /// mutate only runtime health fields so configuration always stays fresh.
    pub fn update_runtime<F: FnOnce(&mut Channel)>(&self, id: i64, mutate: F) -> Result<()> {
        let _guard = self.runtime_mu.lock().unwrap_or_else(|p| p.into_inner());
        let mut fresh = self.get(id)?;
        mutate(&mut fresh);
        self.save_locked(&mut fresh)
    }

    pub(crate) fn record_probe_result<F: FnOnce(&mut Channel)>(
        &self,
        expected: &Channel,
        health: &ModelHealth,
        lease: &crate::key_pool::KeyLease,
        mutate_headline: F,
    ) -> Result<bool> {
        let _guard = self.runtime_mu.lock().unwrap_or_else(|p| p.into_inner());
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let sql = format!("SELECT {CHANNEL_COLUMNS} FROM desktop_channels WHERE id=?");
        let Some(mut fresh) = tx
            .query_row(&sql, [expected.id], scan_channel_row)
            .optional()?
        else {
            return Ok(false);
        };
        let Some(checked_at) = health.checked_at else {
            return Ok(false);
        };
        if !fresh.enabled
            || !fresh.same_upstream(expected)
            || !fresh.has_model(&health.model)
            || !lease.is_current_in(&tx, &health.model)?
        {
            return Ok(false);
        }
        let previous: Option<String> = tx
            .query_row(
                "SELECT checked_at FROM desktop_model_health WHERE channel_id=? AND model=?",
                params![expected.id, health.model],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        if parse_time(previous.as_deref()).is_some_and(|time| time > checked_at) {
            return Ok(false);
        }
        tx.execute(
            "INSERT INTO desktop_model_health(channel_id,model,status,latency_ms,ttft_ms,error,checked_at) VALUES(?,?,?,?,?,?,?) \
             ON CONFLICT(channel_id,model) DO UPDATE SET status=excluded.status,latency_ms=excluded.latency_ms,ttft_ms=excluded.ttft_ms,error=excluded.error,checked_at=excluded.checked_at",
            params![expected.id, health.model, health.status, health.latency_ms, health.ttft_ms, health.error, format_time(checked_at)],
        )?;
        if fresh.primary_model == expected.primary_model
            && fresh.primary_model == health.model
            && !fresh.last_checked_at.is_some_and(|time| time > checked_at)
        {
            mutate_headline(&mut fresh);
            tx.execute(
                "UPDATE desktop_channels SET last_status=?,last_latency_ms=?,last_ttft_ms=?,last_ping_latency_ms=?,last_error=?,last_checked_at=?,probe_streak=?,failure_count=?,circuit_state=?,cooldown_until=? WHERE id=?",
                params![fresh.last_status, fresh.last_latency_ms, fresh.last_ttft_ms, fresh.last_ping_latency_ms, fresh.last_error, fresh.last_checked_at.map(format_time), fresh.probe_streak, fresh.failure_count, fresh.circuit_state, fresh.cooldown_until.map(format_time), fresh.id],
            )?;
        }
        tx.commit()?;
        Ok(true)
    }

    /// Persist diagnostics only while the upstream, model and credential epoch are current.
    pub(crate) fn diagnostic_is_current(
        &self,
        expected: &Channel,
        model: &str,
        lease: &crate::key_pool::KeyLease,
    ) -> Result<bool> {
        let _guard = self.runtime_mu.lock().unwrap_or_else(|p| p.into_inner());
        let conn = self.lock();
        let sql = format!("SELECT {CHANNEL_COLUMNS} FROM desktop_channels WHERE id=?");
        let Some(current) = conn
            .query_row(&sql, [expected.id], scan_channel_row)
            .optional()?
        else {
            return Ok(false);
        };
        Ok(current.enabled
            && current.same_upstream(expected)
            && current.primary_model == expected.primary_model
            && current.has_model(model)
            && lease.is_current_in(&conn, model)?)
    }

    pub fn set_injection(
        &self,
        expected: &Channel,
        report: &crate::probe::InjectionReport,
        lease: &crate::key_pool::KeyLease,
    ) -> Result<bool> {
        let _runtime_guard = self.runtime_mu.lock().unwrap_or_else(|p| p.into_inner());
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let sql = format!("SELECT {CHANNEL_COLUMNS} FROM desktop_channels WHERE id=?");
        let Some(current) = tx
            .query_row(&sql, [expected.id], scan_channel_row)
            .optional()?
        else {
            return Ok(false);
        };
        if !current.enabled
            || !current.same_upstream(expected)
            || !current.has_model(&report.model)
            || current
                .injection_checked_at
                .is_some_and(|time| time > report.checked_at)
            || !lease.is_current_in(&tx, &report.model)?
        {
            return Ok(false);
        }
        tx.execute(
            "UPDATE desktop_channels
             SET injection_state = ?, injection_tokens = ?, injection_content = ?, injection_checked_at = ?, updated_at = ?
             WHERE id = ?",
            params![report.state, report.prompt_tokens, report.content, format_time(report.checked_at), format_time(Utc::now()), expected.id],
        )?;
        tx.commit()?;
        Ok(true)
    }

    pub fn delete(&self, id: i64) -> Result<()> {
        let _runtime_guard = self.runtime_mu.lock().unwrap_or_else(|p| p.into_inner());
        let conn = self.lock();
        let affected = conn.execute("DELETE FROM desktop_channels WHERE id = ?", params![id])?;
        if affected == 0 {
            return Err(StoreError::NotFound(id));
        }
        Ok(())
    }

    // ---- logs -----------------------------------------------------------

    /// Subscribes to log events. Returns the receiver and a subscription id
    /// for [`Store::unsubscribe`].
    pub fn subscribe_logs(&self) -> (mpsc::Receiver<LogEvent>, u64) {
        let (tx, rx) = mpsc::channel(128);
        let id = {
            let mut next = self.next_sub_id.lock().unwrap_or_else(|p| p.into_inner());
            let id = *next;
            *next += 1;
            id
        };
        self.subscribers
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .push((id, tx));
        (rx, id)
    }

    #[cfg(test)]
    pub(crate) fn log_subscriber_count(&self) -> usize {
        self.subscribers.read().unwrap().len()
    }

    pub fn unsubscribe(&self, id: u64) {
        self.subscribers
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|(sub_id, _)| *sub_id != id);
    }

    /// Non-blocking broadcast so slow consumers never block gateway work.
    pub fn broadcast_log(&self, event: LogEvent) {
        let mut subscribers = self.subscribers.write().unwrap_or_else(|p| p.into_inner());
        subscribers.retain(|(_, tx)| match tx.try_send(event.clone()) {
            Err(mpsc::error::TrySendError::Closed(_)) => false,
            Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => true,
        });
    }

    pub fn record_log(&self, log: &mut RequestLog) -> Result<()> {
        if log.created_at.timestamp() == 0 {
            log.created_at = Utc::now();
        }
        if log.total_tokens == 0 && (log.prompt_tokens > 0 || log.completion_tokens > 0) {
            log.total_tokens = log.prompt_tokens + log.completion_tokens;
        }
        {
            let conn = self.lock();
            conn.execute(
                "INSERT INTO desktop_request_logs (
                    model, channel_name, status_code, latency_ms, ttft_ms, prompt_tokens,
                    completion_tokens, total_tokens, is_stream, error_message, failover_from,
                    is_failover, upstream_model, endpoint, client_ip, user_agent,
                    request_path, finish_reason, trace_json, created_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    log.model,
                    log.channel_name,
                    log.status_code,
                    log.latency_ms,
                    log.ttft_ms,
                    log.prompt_tokens,
                    log.completion_tokens,
                    log.total_tokens,
                    log.is_stream as i64,
                    log.error_message,
                    log.failover_from,
                    log.is_failover as i64,
                    log.upstream_model,
                    log.endpoint,
                    log.client_ip,
                    log.user_agent,
                    log.request_path,
                    log.finish_reason,
                    log.trace_json,
                    format_time(log.created_at),
                ],
            )?;
            log.id = conn.last_insert_rowid();
        }
        // Broadcast a copy: the caller keeps mutating the same log while
        // streaming, and subscribers serialize it on their own task.
        self.broadcast_log(LogEvent {
            kind: "created".to_string(),
            log: Some(log.clone()),
        });
        Ok(())
    }

    pub fn update_log(&self, log: &RequestLog) -> Result<()> {
        if log.id <= 0 {
            return Ok(());
        }
        let mut log = log.clone();
        if log.total_tokens == 0 && (log.prompt_tokens > 0 || log.completion_tokens > 0) {
            log.total_tokens = log.prompt_tokens + log.completion_tokens;
        }
        {
            let conn = self.lock();
            conn.execute(
                "UPDATE desktop_request_logs
                 SET channel_name = ?, status_code = ?, latency_ms = ?, ttft_ms = ?, prompt_tokens = ?,
                     completion_tokens = ?, total_tokens = ?, is_stream = ?, error_message = ?, failover_from = ?,
                     is_failover = ?, upstream_model = ?, endpoint = ?, client_ip = ?, user_agent = ?,
                     request_path = ?, finish_reason = ?, trace_json = ?
                 WHERE id = ?",
                params![
                    log.channel_name,
                    log.status_code,
                    log.latency_ms,
                    log.ttft_ms,
                    log.prompt_tokens,
                    log.completion_tokens,
                    log.total_tokens,
                    log.is_stream as i64,
                    log.error_message,
                    log.failover_from,
                    log.is_failover as i64,
                    log.upstream_model,
                    log.endpoint,
                    log.client_ip,
                    log.user_agent,
                    log.request_path,
                    log.finish_reason,
                    log.trace_json,
                    log.id,
                ],
            )?;
        }
        self.broadcast_log(LogEvent {
            kind: "updated".to_string(),
            log: Some(log),
        });
        Ok(())
    }

    pub fn list_logs(&self, limit: i64) -> Result<Vec<RequestLog>> {
        let limit = if limit <= 0 || limit > 500 {
            100
        } else {
            limit
        };
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, model, channel_name, status_code, latency_ms, ttft_ms,
                    prompt_tokens, completion_tokens, total_tokens, is_stream,
                    error_message, failover_from, is_failover,
                    COALESCE(upstream_model, ''), COALESCE(endpoint, ''),
                    COALESCE(client_ip, ''), COALESCE(user_agent, ''),
                    COALESCE(request_path, ''), COALESCE(finish_reason, ''),
                    COALESCE(trace_json, ''), created_at
             FROM desktop_request_logs ORDER BY id DESC LIMIT ?",
        )?;
        let rows = stmt.query_map(params![limit], |row| {
            let created_at: Option<String> = row.get(20)?;
            Ok(RequestLog {
                id: row.get(0)?,
                model: row.get(1)?,
                channel_name: row.get(2)?,
                status_code: row.get(3)?,
                latency_ms: row.get(4)?,
                ttft_ms: row.get(5)?,
                prompt_tokens: row.get(6)?,
                completion_tokens: row.get(7)?,
                total_tokens: row.get(8)?,
                is_stream: row.get::<_, i64>(9)? != 0,
                error_message: row.get(10)?,
                failover_from: row.get(11)?,
                is_failover: row.get::<_, i64>(12)? != 0,
                upstream_model: row.get(13)?,
                endpoint: row.get(14)?,
                client_ip: row.get(15)?,
                user_agent: row.get(16)?,
                request_path: row.get(17)?,
                finish_reason: row.get(18)?,
                trace_json: row.get(19)?,
                created_at: parse_time(created_at.as_deref()).unwrap_or_else(Utc::now),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn clear_logs(&self) -> Result<()> {
        {
            let conn = self.lock();
            conn.execute("DELETE FROM desktop_request_logs", [])?;
        }
        self.broadcast_log(LogEvent {
            kind: "cleared".to_string(),
            log: None,
        });
        Ok(())
    }

    pub fn prune_logs_older_than(&self, days: i64) -> Result<i64> {
        if days <= 0 {
            return Ok(0);
        }
        let cutoff = Utc::now() - chrono::Duration::days(days);
        let affected = {
            let conn = self.lock();
            conn.execute(
                "DELETE FROM desktop_request_logs WHERE created_at < ?",
                params![format_time(cutoff)],
            )? as i64
        };
        if affected > 0 {
            self.broadcast_log(LogEvent {
                kind: "pruned".to_string(),
                log: None,
            });
        }
        Ok(affected)
    }

    /// Trims oldest logs when the total count exceeds `max_count`.
    pub fn prune_logs_limit(&self, max_count: i64) -> Result<i64> {
        if max_count <= 0 {
            return Ok(0);
        }
        let count = self.count_logs()?;
        if count <= max_count {
            return Ok(0);
        }
        let affected = {
            let conn = self.lock();
            conn.execute(
                "DELETE FROM desktop_request_logs WHERE id NOT IN (
                    SELECT id FROM desktop_request_logs ORDER BY id DESC LIMIT ?
                 )",
                params![max_count],
            )? as i64
        };
        if affected > 0 {
            self.broadcast_log(LogEvent {
                kind: "pruned".to_string(),
                log: None,
            });
        }
        Ok(affected)
    }

    /// Applies the configured retention days and max log count rules.
    pub fn auto_prune_logs(&self) -> Result<i64> {
        let retention_days: i64 = self
            .get_setting("log_retention_days", "7")?
            .parse()
            .unwrap_or(7);
        let retention_days = if retention_days < 0 {
            7
        } else {
            retention_days
        };
        let max_count: i64 = self
            .get_setting("log_max_count", "5000")?
            .parse()
            .unwrap_or(5000);
        let max_count = if max_count < 0 { 5000 } else { max_count };

        let mut total = 0;
        if retention_days > 0 {
            total += self.prune_logs_older_than(retention_days).unwrap_or(0);
        }
        if max_count > 0 {
            total += self.prune_logs_limit(max_count).unwrap_or(0);
        }
        Ok(total)
    }

    pub fn count_logs(&self) -> Result<i64> {
        let conn = self.lock();
        let count: i64 =
            conn.query_row("SELECT COUNT(1) FROM desktop_request_logs", [], |row| {
                row.get(0)
            })?;
        Ok(count)
    }

    // ---- settings -------------------------------------------------------

    pub fn get_setting(&self, key: &str, fallback: &str) -> Result<String> {
        let conn = self.lock();
        let value: Option<String> = conn
            .query_row(
                "SELECT value FROM desktop_settings WHERE key = ?",
                params![key],
                |row| row.get(0),
            )
            .optional()?;
        Ok(value.unwrap_or_else(|| fallback.to_string()))
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO desktop_settings (key, value, updated_at) VALUES (?, ?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![key, value, format_time(Utc::now())],
        )?;
        Ok(())
    }

    /// Updates several settings as one SQLite transaction.
    pub fn set_settings(&self, updates: &[(String, String)]) -> Result<()> {
        if updates.is_empty() {
            return Ok(());
        }
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let now = format_time(Utc::now());
        for (key, value) in updates {
            tx.execute(
                "INSERT INTO desktop_settings (key, value, updated_at) VALUES (?, ?, ?)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                params![key, value, now],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    // ---- analytics ------------------------------------------------------

    pub fn get_analytics(&self, days: i64) -> Result<AnalyticsResult> {
        let mut result = AnalyticsResult {
            days,
            summary: AnalyticsSummary::default(),
            daily: Vec::new(),
            models: Vec::new(),
            channels: Vec::new(),
        };

        let cutoff = analytics_cutoff(days);
        let where_sql = match &cutoff {
            Some(_) => " WHERE created_at >= ?",
            None => "",
        };
        let where_extra = |extra: &[&str]| -> String {
            let mut conditions: Vec<String> = Vec::new();
            if cutoff.is_some() {
                conditions.push("created_at >= ?".to_string());
            }
            for cond in extra {
                if !cond.is_empty() {
                    conditions.push((*cond).to_string());
                }
            }
            if conditions.is_empty() {
                String::new()
            } else {
                format!(" WHERE {}", conditions.join(" AND "))
            }
        };
        let args: Vec<&dyn rusqlite::ToSql> = match &cutoff {
            Some(text) => vec![text],
            None => Vec::new(),
        };

        let conn = self.lock();

        // 1. Overall summary.
        let summary_sql = format!(
            "SELECT COUNT(1),
                COALESCE(SUM(CASE WHEN status_code >= 200 AND status_code < 400 THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status_code = 0 OR status_code >= 400 THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(total_tokens), 0),
                COALESCE(SUM(prompt_tokens), 0),
                COALESCE(SUM(completion_tokens), 0),
                COALESCE(AVG(CASE WHEN latency_ms > 0 THEN latency_ms ELSE NULL END), 0),
                COALESCE(AVG(CASE WHEN ttft_ms > 0 THEN ttft_ms ELSE NULL END), 0)
             FROM desktop_request_logs{where_sql}"
        );
        let summary = conn.query_row(&summary_sql, args.as_slice(), |row| {
            Ok(AnalyticsSummary {
                total_requests: row.get(0)?,
                success_requests: row.get(1)?,
                failed_requests: row.get(2)?,
                total_tokens: row.get(3)?,
                prompt_tokens: row.get(4)?,
                completion_tokens: row.get(5)?,
                avg_latency_ms: round_analytics(row.get(6)?),
                avg_ttft_ms: round_analytics(row.get(7)?),
                success_rate: 0.0,
            })
        })?;
        result.summary = summary;
        if result.summary.total_requests > 0 {
            result.summary.success_rate = round_analytics(
                result.summary.success_requests as f64 / result.summary.total_requests as f64
                    * 100.0,
            );
        }

        // 2. Daily trends.
        let daily_sql = format!(
            "SELECT date(created_at, 'localtime') AS day, COUNT(1),
                COALESCE(SUM(CASE WHEN status_code >= 200 AND status_code < 400 THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status_code = 0 OR status_code >= 400 THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(total_tokens), 0),
                COALESCE(SUM(prompt_tokens), 0),
                COALESCE(SUM(completion_tokens), 0),
                COALESCE(AVG(CASE WHEN latency_ms > 0 THEN latency_ms ELSE NULL END), 0),
                COALESCE(AVG(CASE WHEN ttft_ms > 0 THEN ttft_ms ELSE NULL END), 0)
             FROM desktop_request_logs{where_sql}
             GROUP BY day ORDER BY day ASC"
        );
        {
            let mut stmt = conn.prepare(&daily_sql)?;
            let rows = stmt.query_map(args.as_slice(), |row| {
                Ok(DailyAnalyticsTrend {
                    date: row.get(0)?,
                    total_requests: row.get(1)?,
                    success_requests: row.get(2)?,
                    failed_requests: row.get(3)?,
                    total_tokens: row.get(4)?,
                    prompt_tokens: row.get(5)?,
                    completion_tokens: row.get(6)?,
                    avg_latency_ms: round_analytics(row.get(7)?),
                    avg_ttft_ms: round_analytics(row.get(8)?),
                })
            })?;
            result.daily = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        }

        // 3. Top channel per model.
        let top_where = where_extra(&["channel_name != ''", "channel_name != '路由中...'"]);
        // The extra conditions are inlined literals, so only the cutoff is bound.
        let top_args: Vec<&dyn rusqlite::ToSql> = match &cutoff {
            Some(text) => vec![text],
            None => Vec::new(),
        };
        let mut top_channels: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        {
            let top_sql = format!(
                "SELECT model, channel_name, COUNT(1) as cnt
                 FROM desktop_request_logs{top_where}
                 GROUP BY model, channel_name ORDER BY model, cnt DESC"
            );
            if let Ok(mut stmt) = conn.prepare(&top_sql) {
                let rows = stmt.query_map(top_args.as_slice(), |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                });
                if let Ok(rows) = rows {
                    for row in rows.flatten() {
                        let (model, channel) = row;
                        if !channel.is_empty() {
                            top_channels.entry(model).or_insert(channel);
                        }
                    }
                }
            }
        }

        // 4. Per-model stats.
        let model_sql = format!(
            "SELECT model, COUNT(1),
                COALESCE(SUM(CASE WHEN status_code >= 200 AND status_code < 400 THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status_code = 0 OR status_code >= 400 THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(total_tokens), 0),
                COALESCE(SUM(prompt_tokens), 0),
                COALESCE(SUM(completion_tokens), 0),
                COALESCE(AVG(CASE WHEN latency_ms > 0 THEN latency_ms ELSE NULL END), 0),
                COALESCE(MIN(CASE WHEN latency_ms > 0 THEN latency_ms ELSE NULL END), 0),
                COALESCE(MAX(CASE WHEN latency_ms > 0 THEN latency_ms ELSE NULL END), 0),
                COALESCE(AVG(CASE WHEN ttft_ms > 0 THEN ttft_ms ELSE NULL END), 0)
             FROM desktop_request_logs{where_sql}
             GROUP BY model ORDER BY COUNT(1) DESC"
        );
        {
            let mut stmt = conn.prepare(&model_sql)?;
            let rows = stmt.query_map(args.as_slice(), |row| {
                let total: i64 = row.get(1)?;
                let success: i64 = row.get(2)?;
                Ok(ModelAnalyticsStat {
                    model: row.get(0)?,
                    total_requests: total,
                    success_requests: success,
                    failed_requests: row.get(3)?,
                    success_rate: if total > 0 {
                        round_analytics(success as f64 / total as f64 * 100.0)
                    } else {
                        0.0
                    },
                    total_tokens: row.get(4)?,
                    prompt_tokens: row.get(5)?,
                    completion_tokens: row.get(6)?,
                    avg_latency_ms: round_analytics(row.get(7)?),
                    min_latency_ms: row.get(8)?,
                    max_latency_ms: row.get(9)?,
                    avg_ttft_ms: round_analytics(row.get(10)?),
                    top_channel: String::new(),
                })
            })?;
            let mut models = rows.collect::<rusqlite::Result<Vec<_>>>()?;
            for model in &mut models {
                model.top_channel = top_channels.get(&model.model).cloned().unwrap_or_default();
            }
            result.models = models;
        }

        // 5. Per-channel stats.
        let chan_args: Vec<&dyn rusqlite::ToSql> = match &cutoff {
            Some(text) => vec![text],
            None => Vec::new(),
        };
        let chan_sql = format!(
            "SELECT channel_name, COUNT(1),
                COALESCE(SUM(CASE WHEN status_code >= 200 AND status_code < 400 THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status_code = 0 OR status_code >= 400 THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(total_tokens), 0),
                COALESCE(AVG(CASE WHEN latency_ms > 0 THEN latency_ms ELSE NULL END), 0),
                COALESCE(AVG(CASE WHEN ttft_ms > 0 THEN ttft_ms ELSE NULL END), 0)
             FROM desktop_request_logs{top_where}
             GROUP BY channel_name ORDER BY COUNT(1) DESC"
        );
        {
            let mut stmt = conn.prepare(&chan_sql)?;
            let rows = stmt.query_map(chan_args.as_slice(), |row| {
                let total: i64 = row.get(1)?;
                let success: i64 = row.get(2)?;
                Ok(ChannelAnalyticsStat {
                    channel_name: row.get(0)?,
                    total_requests: total,
                    success_requests: success,
                    failed_requests: row.get(3)?,
                    success_rate: if total > 0 {
                        round_analytics(success as f64 / total as f64 * 100.0)
                    } else {
                        0.0
                    },
                    total_tokens: row.get(4)?,
                    avg_latency_ms: round_analytics(row.get(5)?),
                    avg_ttft_ms: round_analytics(row.get(6)?),
                })
            })?;
            result.channels = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        }

        Ok(result)
    }
}

/// The cadence the scheduler will actually use; always 0 because automatic
/// probes are disabled in the desktop edition.
pub fn effective_interval_seconds() -> i64 {
    0
}

/// Copies the user-editable configuration fields from `src` to `dst`, leaving
/// every runtime health field untouched.
pub fn copy_config_state(dst: &mut Channel, src: &Channel) {
    dst.name = src.name.clone();
    dst.provider = src.provider.clone();
    dst.api_mode = src.api_mode.clone();
    dst.endpoint = src.endpoint.clone();
    dst.api_key = src.api_key.clone();
    dst.primary_model = src.primary_model.clone();
    dst.extra_models = src.extra_models.clone();
    dst.enabled = src.enabled;
    dst.interval_seconds = src.interval_seconds;
}

/// Analytics summary for the selected window.
#[derive(Debug, Clone, Default, Serialize)]
pub struct AnalyticsSummary {
    pub total_requests: i64,
    pub success_requests: i64,
    pub failed_requests: i64,
    pub success_rate: f64,
    pub total_tokens: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub avg_latency_ms: f64,
    pub avg_ttft_ms: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DailyAnalyticsTrend {
    pub date: String,
    pub total_requests: i64,
    pub success_requests: i64,
    pub failed_requests: i64,
    pub total_tokens: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub avg_latency_ms: f64,
    pub avg_ttft_ms: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelAnalyticsStat {
    pub model: String,
    pub total_requests: i64,
    pub success_requests: i64,
    pub failed_requests: i64,
    pub success_rate: f64,
    pub total_tokens: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub avg_latency_ms: f64,
    pub min_latency_ms: i64,
    pub max_latency_ms: i64,
    pub avg_ttft_ms: f64,
    pub top_channel: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChannelAnalyticsStat {
    pub channel_name: String,
    pub total_requests: i64,
    pub success_requests: i64,
    pub failed_requests: i64,
    pub success_rate: f64,
    pub total_tokens: i64,
    pub avg_latency_ms: f64,
    pub avg_ttft_ms: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnalyticsResult {
    pub days: i64,
    pub summary: AnalyticsSummary,
    pub daily: Vec<DailyAnalyticsTrend>,
    pub models: Vec<ModelAnalyticsStat>,
    pub channels: Vec<ChannelAnalyticsStat>,
}

fn round_analytics(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

/// The UTC cutoff for `days == 1` (local midnight today) or `days > 1` (local
/// midnight `days - 1` days ago). `days <= 0` means no cutoff.
fn analytics_cutoff(days: i64) -> Option<String> {
    if days <= 0 {
        return None;
    }
    let now = Local::now();
    let midnight = now.date_naive().and_hms_opt(0, 0, 0)?;
    let local = Local.from_local_datetime(&midnight).single()?;
    let start = if days == 1 {
        local
    } else {
        local - chrono::Duration::days(days - 1)
    };
    Some(format_time(start.with_timezone(&Utc)))
}

/// Decodes the `extra_models_json` column. Go marshals a nil slice as `null`
/// and reads it back as an empty list, so a legacy row holding `null` (or any
/// other non-array value) must degrade to "no extra models" instead of failing
/// the whole store — a single old row must not stop the gateway from opening.
fn decode_extra_models(raw: Option<&str>) -> Vec<String> {
    raw.map(str::trim)
        .filter(|text| !text.is_empty())
        .and_then(|text| serde_json::from_str::<Vec<String>>(text).ok())
        .unwrap_or_default()
}

fn scan_channel_row(row: &Row<'_>) -> rusqlite::Result<Channel> {
    let primary_model: String = row.get(6)?;
    let extra_json: Option<String> = row.get(7)?;
    let enabled: i64 = row.get(8)?;
    let last_checked: Option<String> = row.get(15)?;
    let cooldown: Option<String> = row.get(19)?;
    let inject_checked: Option<String> = row.get(23)?;
    let created: Option<String> = row.get(24)?;
    let updated: Option<String> = row.get(25)?;
    let mut seen = HashSet::new();
    let extra_models: Vec<String> = decode_extra_models(extra_json.as_deref())
        .into_iter()
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty() && m != primary_model.trim() && seen.insert(m.clone()))
        .collect();

    let circuit_state: String = row.get(18)?;
    Ok(Channel {
        id: row.get(0)?,
        name: row.get(1)?,
        provider: row.get(2)?,
        api_mode: row.get(3)?,
        endpoint: row.get(4)?,
        api_key: row.get(5)?,
        primary_model,
        extra_models,
        enabled: enabled != 0,
        interval_seconds: row.get(9)?,
        last_status: row.get(10)?,
        last_latency_ms: row.get(11)?,
        last_ttft_ms: row.get(12)?,
        last_ping_latency_ms: row.get(13)?,
        last_error: row.get(14)?,
        last_checked_at: parse_time(last_checked.as_deref()),
        probe_streak: row.get(16)?,
        failure_count: row.get(17)?,
        circuit_state: if circuit_state.is_empty() {
            "closed".to_string()
        } else {
            circuit_state
        },
        cooldown_until: parse_time(cooldown.as_deref()),
        injection_state: row.get(20)?,
        injection_tokens: row.get(21)?,
        injection_content: row.get(22)?,
        injection_checked_at: parse_time(inject_checked.as_deref()),
        created_at: parse_time(created.as_deref()),
        updated_at: parse_time(updated.as_deref()),
        effective_interval_seconds: effective_interval_seconds(),
        model_health: Vec::new(),
        model_performance: Vec::new(),
    })
}

/// Parses an RFC3339 timestamp, tolerating absent or malformed values.
pub fn parse_time(value: Option<&str>) -> Option<DateTime<Utc>> {
    let value = value?;
    if value.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Formats a timestamp the way Go's `time.RFC3339Nano` does: UTC, `Z` suffix,
/// fractional seconds only when non-zero, trailing zeros trimmed.
pub fn format_time(value: DateTime<Utc>) -> String {
    let base = value.format("%Y-%m-%dT%H:%M:%S").to_string();
    let nanos = value.timestamp_subsec_nanos();
    if nanos == 0 {
        return format!("{base}Z");
    }
    let mut frac = format!("{nanos:09}");
    while frac.ends_with('0') {
        frac.pop();
    }
    format!("{base}.{frac}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn injection_report(
        model: &str,
        state: &str,
        prompt_tokens: i64,
        content: &str,
        checked_at: DateTime<Utc>,
    ) -> crate::probe::InjectionReport {
        crate::probe::InjectionReport {
            model: model.into(),
            state: state.into(),
            prompt_tokens,
            content: content.into(),
            message: String::new(),
            checked_at,
        }
    }

    #[test]
    fn upstream_edits_reset_runtime_preserve_identity_and_fence_old_leases() {
        use crate::{
            key_pool::KeyPool,
            resilience::{FailureClass, FailureInfo},
        };
        for field in ["endpoint", "provider", "api_mode"] {
            let store = Store::open_in_memory().unwrap();
            let mut channel = channel("source");
            channel.enabled = true;
            store.save(&mut channel).unwrap();
            let pool = KeyPool::new(store.clone());
            let old = pool
                .acquire_for_channel(&channel, &channel.primary_model, &HashSet::new(), None)
                .unwrap()
                .unwrap();
            old.failure(
                &FailureInfo::new(FailureClass::RateLimit),
                &channel.primary_model,
            )
            .unwrap();
            let auth = pool
                .acquire_probe_for_channel(&channel, &channel.primary_model, None)
                .unwrap()
                .unwrap();
            auth.failure(
                &crate::resilience::classify_status(401),
                &channel.primary_model,
            )
            .unwrap();
            channel.last_status = "error".into();
            channel.last_error = "old upstream".into();
            channel.last_latency_ms = Some(500);
            channel.last_ttft_ms = Some(100);
            channel.last_checked_at = Some(Utc::now());
            channel.cooldown_until = Some(Utc::now() + chrono::Duration::minutes(5));
            channel.circuit_state = "open".into();
            channel.failure_count = 3;
            channel.injection_state = "detected".into();
            channel.injection_tokens = Some(500);
            store.save(&mut channel).unwrap();
            store
                .set_model_health(
                    channel.id,
                    &ModelHealth {
                        model: channel.primary_model.clone(),
                        status: "error".into(),
                        ..Default::default()
                    },
                )
                .unwrap();
            store
                .record_model_ttft(&channel, &channel.primary_model, 100, Utc::now())
                .unwrap();
            let before = store.list_keys(channel.id).unwrap().remove(0);
            channel.name = "renamed".into();
            store.save(&mut channel).unwrap();
            assert!(store.list_keys(channel.id).unwrap()[0].auth_invalid);
            assert!(store.get(channel.id).unwrap().cooldown_until.is_some());
            let previous = channel.clone();
            match field {
                "endpoint" => channel.endpoint = "https://new.test".into(),
                "provider" => channel.provider = "anthropic".into(),
                _ => channel.api_mode = "responses".into(),
            }
            store.save(&mut channel).unwrap();
            let fresh = store.get(channel.id).unwrap();
            assert_eq!(fresh.last_status, "unknown");
            assert!(fresh.last_error.is_empty());
            assert_eq!(fresh.failure_count, 0);
            assert!(fresh.cooldown_until.is_none());
            assert_eq!(fresh.circuit_state, "closed");
            assert_eq!(fresh.injection_state, "unknown");
            assert!(fresh.model_health.is_empty() && fresh.model_performance.is_empty());
            let after = store.list_keys(channel.id).unwrap().remove(0);
            assert_eq!(
                (
                    after.id,
                    after.name,
                    after.enabled,
                    after.secret,
                    after.attempts,
                    after.rate_limits
                ),
                (
                    before.id,
                    before.name,
                    before.enabled,
                    before.secret,
                    before.attempts,
                    before.rate_limits
                )
            );
            assert!(!after.auth_invalid && after.cooldown_until.is_none());
            assert!(after.generation > before.generation);
            assert!(!old.is_current(&channel.primary_model).unwrap());
            assert!(pool
                .acquire_for_channel(&previous, &channel.primary_model, &HashSet::new(), None)
                .unwrap()
                .is_none());
            old.failure(
                &FailureInfo::new(FailureClass::RateLimit),
                &channel.primary_model,
            )
            .unwrap();
            old.success(100, 20, &channel.primary_model).unwrap();
            assert!(pool
                .retry_after_model(channel.id, &channel.primary_model)
                .is_zero());
            assert!(pool
                .acquire_for_channel(&fresh, &channel.primary_model, &HashSet::new(), None)
                .unwrap()
                .is_some());
            assert_eq!(
                store.list_keys(channel.id).unwrap()[0].successes,
                before.successes
            );
        }
    }

    #[test]
    fn diagnostics_cannot_overwrite_newer_live_health_or_edited_upstreams() {
        use crate::{
            key_pool::KeyPool,
            resilience::{FailureClass, FailureInfo},
        };
        let store = Store::open_in_memory().unwrap();
        let mut channel = channel("source");
        channel.enabled = true;
        store.save(&mut channel).unwrap();
        let pool = KeyPool::new(store.clone());
        let lease = pool
            .acquire_probe_for_channel(&channel, &channel.primary_model, None)
            .unwrap()
            .unwrap();
        let health = ModelHealth {
            model: channel.primary_model.clone(),
            status: "operational".into(),
            checked_at: Some(Utc::now() - chrono::Duration::seconds(2)),
            ..Default::default()
        };
        store
            .record_model_outcome(
                &channel,
                &channel.primary_model,
                false,
                "new live failure",
                Utc::now(),
            )
            .unwrap();
        assert!(!store
            .record_probe_result(&channel, &health, &lease, |_| panic!("stale headline"))
            .unwrap());
        lease
            .failure(
                &FailureInfo::new(FailureClass::RateLimit),
                &channel.primary_model,
            )
            .unwrap();
        let newer = ModelHealth {
            checked_at: Some(Utc::now()),
            ..health.clone()
        };
        assert!(!store
            .record_probe_result(&channel, &newer, &lease, |_| panic!("old epoch"))
            .unwrap());
        let fresh_lease = pool
            .acquire_probe_for_channel(&channel, &channel.primary_model, None)
            .unwrap()
            .unwrap();
        let old_channel = channel.clone();
        channel.endpoint = "https://new.test".into();
        store.save(&mut channel).unwrap();
        assert!(!store
            .record_probe_result(&old_channel, &newer, &fresh_lease, |_| panic!(
                "old upstream"
            ))
            .unwrap());
        assert!(!store
            .set_injection(
                &old_channel,
                &injection_report(&channel.primary_model, "detected", 500, "old", Utc::now(),),
                &fresh_lease,
            )
            .unwrap());
        let fresh = pool
            .acquire_probe_for_channel(&channel, &channel.primary_model, None)
            .unwrap()
            .unwrap();
        assert!(store
            .set_injection(
                &channel,
                &injection_report(&channel.primary_model, "clean", 20, "", Utc::now(),),
                &fresh,
            )
            .unwrap());
        assert!(!store
            .set_injection(
                &channel,
                &injection_report(
                    &channel.primary_model,
                    "detected",
                    500,
                    "old",
                    Utc::now() - chrono::Duration::seconds(2),
                ),
                &fresh,
            )
            .unwrap());
        assert_eq!(store.get(channel.id).unwrap().injection_state, "clean");
        assert!(store.get(channel.id).unwrap().model_health.is_empty());
    }

    #[test]
    fn cosmetic_endpoint_edits_keep_cooldowns_but_removed_models_fence_old_replies() {
        use crate::{
            key_pool::KeyPool,
            resilience::{FailureClass, FailureInfo},
        };
        let store = Store::open_in_memory().unwrap();
        let mut channel = channel("source");
        channel.enabled = true;
        channel.extra_models = vec!["extra".into()];
        store.save(&mut channel).unwrap();
        let pool = KeyPool::new(store.clone());
        let lease = pool
            .acquire_for_channel(&channel, "extra", &HashSet::new(), None)
            .unwrap()
            .unwrap();
        let old = pool
            .acquire_for_channel(&channel, "extra", &HashSet::new(), None)
            .unwrap()
            .unwrap();
        lease
            .failure(&FailureInfo::new(FailureClass::RateLimit), "extra")
            .unwrap();
        channel.endpoint += "/v1/";
        store.save(&mut channel).unwrap();
        assert!(!pool.retry_after_model(channel.id, "extra").is_zero());
        channel.extra_models.clear();
        store.save(&mut channel).unwrap();
        assert!(pool.retry_after_model(channel.id, "extra").is_zero());
        channel.extra_models.push("extra".into());
        store.save(&mut channel).unwrap();
        assert!(!old.is_current("extra").unwrap());
        old.failure(&FailureInfo::new(FailureClass::RateLimit), "extra")
            .unwrap();
        assert!(pool
            .acquire_for_channel(&channel, "extra", &HashSet::new(), None)
            .unwrap()
            .is_some());
    }

    #[test]
    fn live_ttft_is_smoothed_isolated_and_keeps_probe_data() {
        let store = Store::open_in_memory().unwrap();
        let mut ch = channel("one");
        ch.extra_models = vec!["extra".into()];
        store.save(&mut ch).unwrap();
        let now = Utc::now();
        store
            .set_model_health(
                ch.id,
                &ModelHealth {
                    model: "extra".into(),
                    ttft_ms: Some(900),
                    status: "operational".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        store.record_model_ttft(&ch, "extra", 100, now).unwrap();
        store.record_model_ttft(&ch, "extra", 300, now).unwrap();
        let saved = store.get(ch.id).unwrap();
        let p = saved.fresh_model_performance("extra").unwrap();
        assert_eq!(p.ttft_ms, 140.0);
        assert_eq!(p.samples, 2);
        assert_eq!(p.last_ttft_ms, 300);
        assert_eq!(saved.model_health[0].ttft_ms, Some(900));
        assert!(saved.fresh_model_performance(&ch.primary_model).is_none());
        let mut other = channel("two");
        store.save(&mut other).unwrap();
        assert!(store.get(other.id).unwrap().model_performance.is_empty());
        store.record_model_ttft(&ch, "extra", 0, now).unwrap();
        store.record_model_ttft(&ch, "removed", 100, now).unwrap();
        assert_eq!(store.get(ch.id).unwrap().model_performance[0].samples, 2);
    }

    #[test]
    fn live_ttft_expires_resets_and_rejects_old_configuration() {
        let store = Store::open_in_memory().unwrap();
        let mut ch = channel("one");
        store.save(&mut ch).unwrap();
        let now = Utc::now();
        store
            .record_model_ttft(
                &ch,
                &ch.primary_model,
                100,
                now - chrono::Duration::minutes(31),
            )
            .unwrap();
        assert!(store
            .get(ch.id)
            .unwrap()
            .fresh_model_performance(&ch.primary_model)
            .is_none());
        store
            .record_model_ttft(&ch, &ch.primary_model, 400, now)
            .unwrap();
        let saved = store.get(ch.id).unwrap();
        assert_eq!(saved.model_performance[0].ttft_ms, 400.0);
        assert_eq!(saved.model_performance[0].samples, 1);
        ch.endpoint = "https://changed.test".into();
        store.save(&mut ch).unwrap();
        store
            .record_model_ttft(&saved, &saved.primary_model, 200, now)
            .unwrap();
        assert!(store.get(ch.id).unwrap().model_performance.is_empty());
        store
            .record_model_ttft(&ch, &ch.primary_model, 200, now)
            .unwrap();
        ch.primary_model = "replacement".into();
        store.save(&mut ch).unwrap();
        assert!(store.get(ch.id).unwrap().model_performance.is_empty());
    }

    #[test]
    fn model_order_changes_preserve_channel_diagnostics_and_model_state() {
        for has_health in [false, true] {
            let store = Store::open_in_memory().unwrap();
            let mut ch = channel("source");
            let old_model = ch.primary_model.clone();
            let now = Utc::now();
            ch.extra_models = vec!["extra".into()];
            ch.last_status = "operational".into();
            ch.last_latency_ms = Some(900);
            ch.last_ttft_ms = Some(800);
            ch.last_ping_latency_ms = Some(100);
            ch.last_checked_at = Some(now);
            ch.probe_streak = 4;
            ch.failure_count = 3;
            ch.circuit_state = "open".into();
            ch.cooldown_until = Some(now + chrono::Duration::minutes(5));
            ch.injection_state = "detected".into();
            ch.injection_tokens = Some(900);
            ch.injection_content = "old primary content".into();
            ch.injection_checked_at = Some(now);
            store.save(&mut ch).unwrap();
            store
                .set_model_health(
                    ch.id,
                    &ModelHealth {
                        model: old_model.clone(),
                        status: "operational".into(),
                        latency_ms: Some(900),
                        ttft_ms: Some(800),
                        checked_at: Some(now),
                        ..Default::default()
                    },
                )
                .unwrap();
            if has_health {
                store
                    .set_model_health(
                        ch.id,
                        &ModelHealth {
                            model: "extra".into(),
                            status: "degraded".into(),
                            latency_ms: Some(200),
                            ttft_ms: Some(150),
                            error: "slow extra model".into(),
                            checked_at: Some(now),
                        },
                    )
                    .unwrap();
            }
            store.record_model_ttft(&ch, &old_model, 80, now).unwrap();
            store.record_model_ttft(&ch, "extra", 15, now).unwrap();
            let pool = crate::key_pool::KeyPool::new(store.clone());
            let lease = pool
                .acquire_probe_for_channel(&ch, "extra", None)
                .unwrap()
                .unwrap();
            lease
                .failure(
                    &crate::resilience::FailureInfo::new(
                        crate::resilience::FailureClass::RateLimit,
                    ),
                    "extra",
                )
                .unwrap();
            ch.primary_model = "extra".into();
            ch.extra_models = vec![old_model.clone()];
            store.save(&mut ch).unwrap();
            let saved = store.get(ch.id).unwrap();
            assert_eq!(saved.last_status, "operational");
            assert_eq!(saved.last_latency_ms, Some(900));
            assert_eq!(saved.last_ttft_ms, Some(800));
            assert!(saved.last_error.is_empty());
            assert_eq!(saved.last_checked_at, Some(now));
            assert_eq!(saved.last_ping_latency_ms, Some(100));
            assert_eq!(saved.probe_streak, 4);
            assert_eq!(saved.injection_state, "detected");
            assert_eq!(saved.injection_tokens, Some(900));
            assert_eq!(saved.injection_checked_at, Some(now));
            assert_eq!(saved.injection_content, "old primary content");
            assert_eq!(saved.failure_count, 3);
            assert_eq!(saved.circuit_state, "open");
            assert_eq!(
                saved.cooldown_until,
                Some(now + chrono::Duration::minutes(5))
            );
            assert_eq!(saved.model_health.len(), if has_health { 2 } else { 1 });
            assert_eq!(saved.model_performance.len(), 2);
            assert!(!pool.retry_after_model(ch.id, "extra").is_zero());
        }
    }

    #[test]
    fn secondary_diagnostics_require_current_configuration_and_credential_epochs() {
        for change in [
            "none", "key", "model", "upstream", "disabled", "primary", "epoch",
        ] {
            let store = Store::open_in_memory().unwrap();
            let mut ch = channel("source");
            ch.extra_models = vec!["extra".into()];
            store.save(&mut ch).unwrap();
            let expected = ch.clone();
            let pool = crate::key_pool::KeyPool::new(store.clone());
            let lease = pool
                .acquire_probe_for_channel(&ch, "extra", None)
                .unwrap()
                .unwrap();
            match change {
                "key" => {
                    store
                        .patch_key(
                            ch.id,
                            lease.key.id,
                            &crate::store_keys::KeyPatch {
                                secret: Some("replacement".into()),
                                ..Default::default()
                            },
                        )
                        .unwrap();
                }
                "model" => {
                    ch.extra_models.clear();
                    store.save(&mut ch).unwrap();
                    ch.extra_models.push("extra".into());
                    store.save(&mut ch).unwrap();
                }
                "upstream" => {
                    ch.endpoint = "https://new.test".into();
                    store.save(&mut ch).unwrap();
                }
                "disabled" => {
                    ch.enabled = false;
                    store.save(&mut ch).unwrap();
                }
                "primary" => {
                    ch.extra_models.push(ch.primary_model.clone());
                    ch.primary_model = "extra".into();
                    store.save(&mut ch).unwrap();
                }
                "epoch" => {
                    lease
                        .failure(
                            &crate::resilience::FailureInfo::new(
                                crate::resilience::FailureClass::RateLimit,
                            ),
                            "extra",
                        )
                        .unwrap();
                }
                _ => {}
            }
            assert_eq!(
                store
                    .diagnostic_is_current(&expected, "extra", &lease)
                    .unwrap(),
                change == "none",
                "{change}"
            );
        }
    }

    fn channel(name: &str) -> Channel {
        Channel {
            name: name.to_string(),
            provider: "openai".to_string(),
            api_mode: "chat_completions".to_string(),
            endpoint: "https://api.example.com".to_string(),
            api_key: "sk-test".to_string(),
            primary_model: "gpt-4o-mini".to_string(),
            enabled: true,
            ..Default::default()
        }
    }

    #[test]
    fn save_and_get_roundtrip() {
        let store = Store::open_in_memory().unwrap();
        let mut ch = channel("alpha");
        store.save(&mut ch).unwrap();
        assert!(ch.id > 0);

        let got = store.get(ch.id).unwrap();
        assert_eq!(got.name, "alpha");
        assert_eq!(got.provider, "openai");
        assert_eq!(got.primary_model, "gpt-4o-mini");
        assert!(got.enabled);
        assert_eq!(got.circuit_state, "closed");
        assert!(got.created_at.is_some());
    }

    #[test]
    fn get_missing_channel_reports_not_found() {
        let store = Store::open_in_memory().unwrap();
        match store.get(404) {
            Err(StoreError::NotFound(id)) => assert_eq!(id, 404),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn save_extra_models_and_prune_stale_health() {
        let store = Store::open_in_memory().unwrap();
        let mut ch = channel("beta");
        ch.extra_models = vec!["gpt-4o".to_string(), "gpt-4o-mini-2".to_string()];
        store.save(&mut ch).unwrap();

        store
            .set_model_health(
                ch.id,
                &ModelHealth {
                    model: "stale-model".to_string(),
                    status: "operational".to_string(),
                    ..Default::default()
                },
            )
            .unwrap();
        // Re-saving prunes health rows for models no longer configured.
        store.save(&mut ch).unwrap();

        let got = store.get(ch.id).unwrap();
        assert_eq!(
            got.extra_models,
            vec!["gpt-4o".to_string(), "gpt-4o-mini-2".to_string()]
        );
        assert!(
            got.model_health.is_empty(),
            "stale health row should be pruned: {:?}",
            got.model_health
        );
    }

    #[test]
    fn list_orders_by_id() {
        let store = Store::open_in_memory().unwrap();
        for name in ["one", "two", "three"] {
            let mut ch = channel(name);
            store.save(&mut ch).unwrap();
        }
        let names: Vec<String> = store.list().unwrap().into_iter().map(|c| c.name).collect();
        assert_eq!(names, vec!["one", "two", "three"]);
    }

    #[test]
    fn record_and_list_logs() {
        let store = Store::open_in_memory().unwrap();
        let mut log = RequestLog {
            model: "gpt-4o-mini".to_string(),
            channel_name: "alpha".to_string(),
            status_code: 200,
            latency_ms: 120,
            prompt_tokens: 10,
            completion_tokens: 20,
            ..Default::default()
        };
        store.record_log(&mut log).unwrap();
        // Total tokens are derived when both parts are present.
        assert_eq!(log.total_tokens, 30);

        let logs = store.list_logs(100).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].total_tokens, 30);
        assert_eq!(logs[0].channel_name, "alpha");
    }

    #[test]
    fn settings_roundtrip_with_fallback() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(
            store.get_setting("missing", "fallback").unwrap(),
            "fallback"
        );
        store.set_setting("log_max_count", "123").unwrap();
        assert_eq!(store.get_setting("log_max_count", "x").unwrap(), "123");
    }

    #[test]
    fn analytics_summary_counts_requests() {
        let store = Store::open_in_memory().unwrap();
        for status in [200, 200, 500] {
            let mut log = RequestLog {
                model: "gpt-4o-mini".to_string(),
                channel_name: "alpha".to_string(),
                status_code: status,
                latency_ms: 100,
                prompt_tokens: 5,
                completion_tokens: 5,
                ..Default::default()
            };
            store.record_log(&mut log).unwrap();
        }
        let analytics = store.get_analytics(7).unwrap();
        assert_eq!(analytics.summary.total_requests, 3);
        assert_eq!(analytics.summary.success_requests, 2);
        assert_eq!(analytics.summary.failed_requests, 1);
        assert_eq!(analytics.summary.total_tokens, 30);
        assert_eq!(analytics.models.len(), 1);
        assert_eq!(analytics.models[0].top_channel, "alpha");
    }

    #[test]
    fn time_formatting_matches_go_rfc3339nano() {
        let dt: DateTime<Utc> = "2026-09-30T14:11:22Z".parse().unwrap();
        assert_eq!(format_time(dt), "2026-09-30T14:11:22Z");

        let with_frac: DateTime<Utc> = "2026-09-30T14:11:22.120Z".parse().unwrap();
        assert_eq!(format_time(with_frac), "2026-09-30T14:11:22.12Z");

        assert_eq!(
            parse_time(Some("2026-09-30T14:11:22.12Z")).map(format_time),
            Some("2026-09-30T14:11:22.12Z".to_string())
        );
        assert_eq!(parse_time(None), None);
        assert_eq!(parse_time(Some("")), None);
    }

    #[test]
    fn decodes_legacy_extra_models() {
        assert_eq!(decode_extra_models(Some("null")), Vec::<String>::new());
        assert_eq!(decode_extra_models(None), Vec::<String>::new());
        assert_eq!(decode_extra_models(Some("  ")), Vec::<String>::new());
        assert_eq!(decode_extra_models(Some("not json")), Vec::<String>::new());
        assert_eq!(
            decode_extra_models(Some("[\"a\",\"b\"]")),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn opens_a_database_with_a_legacy_null_extra_models_row() {
        // Go marshals a nil slice as the JSON literal `null`; opening such a
        // database used to abort startup because the health prune parsed it
        // strictly. It must now open and report no extra models.
        let path = std::env::temp_dir().join(format!(
            "heigate-legacy-null-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));

        {
            let store = Store::open(&path).unwrap();
            let mut ch = channel("legacy");
            store.save(&mut ch).unwrap();
        }
        {
            // Rewrite the column the way the Go implementation would have.
            let conn = Connection::open(&path).unwrap();
            conn.execute("UPDATE desktop_channels SET extra_models_json = 'null'", [])
                .unwrap();
        }

        let store = Store::open(&path).expect("store must open despite the legacy row");
        let channels = store.list().unwrap();
        assert_eq!(channels.len(), 1);
        assert!(channels[0].extra_models.is_empty());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn deduplicates_and_excludes_primary_model_from_extra_models() {
        let store = Store::open_in_memory().unwrap();
        let mut ch = channel("test-dedup");
        ch.primary_model = "claude-sonnet-4-6".to_string();
        ch.extra_models = vec![
            "claude-sonnet-4-6".to_string(),
            "claude-3-haiku".to_string(),
            "claude-3-haiku".to_string(),
        ];
        store.save(&mut ch).unwrap();

        let loaded = store.get(ch.id).unwrap();
        assert_eq!(loaded.primary_model, "claude-sonnet-4-6");
        assert_eq!(loaded.extra_models, vec!["claude-3-haiku".to_string()]);
    }

    #[test]
    fn normalizes_models_before_persisting() {
        let store = Store::open_in_memory().unwrap();
        let mut ch = channel("test-normalize");
        ch.primary_model = "  gpt-4o  ".to_string();
        ch.extra_models = vec![" gpt-4o-mini ".to_string(), "gpt-4o-mini".to_string()];

        store.save(&mut ch).unwrap();

        assert_eq!(ch.primary_model, "gpt-4o");
        assert_eq!(ch.extra_models, vec!["gpt-4o-mini".to_string()]);
        let loaded = store.get(ch.id).unwrap();
        assert_eq!(loaded.primary_model, "gpt-4o");
        assert_eq!(loaded.extra_models, vec!["gpt-4o-mini".to_string()]);
    }
}
