//! Credentials have stable identities independent of their position in a pool.
//! Runtime updates never write channel configuration or resurrect deleted keys.
use std::path::Path;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::store::{Result, Store, StoreError};

#[cfg(test)]
#[path = "store_keys_tests.rs"]
mod tests;

#[derive(Clone, Serialize, Deserialize)]
pub struct ChannelKey {
    pub id: i64,
    pub channel_id: i64,
    pub name: String,
    #[serde(skip_serializing, default)]
    pub secret: String,
    pub enabled: bool,
    pub position: i64,
    pub auth_invalid: bool,
    pub rate_limit_count: i64,
    pub cooldown_until: Option<DateTime<Utc>>,
    pub last_error: String,
    pub last_used_at: Option<DateTime<Utc>>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub attempts: i64,
    pub successes: i64,
    pub rate_limits: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub generation: i64,
}

#[derive(Clone, Deserialize)]
pub struct KeyInput {
    #[serde(default)]
    pub name: String,
    pub secret: String,
}

#[derive(Default, Deserialize)]
pub struct KeyPatch {
    pub name: Option<String>,
    pub secret: Option<String>,
    pub enabled: Option<bool>,
    pub position: Option<i64>,
}

pub fn backup_before_migration(conn: &Connection, path: &Path) -> Result<()> {
    let old: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='desktop_channels') AND NOT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='channel_keys')",
        [], |r| r.get(0),
    )?;
    if old {
        // VACUUM INTO includes committed WAL pages, unlike copying the live file.
        let backup = path.with_extension(format!(
            "pre-key-pool-{}.sqlite",
            Utc::now().timestamp_millis()
        ));
        conn.execute("VACUUM INTO ?", [backup.to_string_lossy().as_ref()])?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&backup, std::fs::Permissions::from_mode(0o600))
                .map_err(|_| StoreError::Invalid("cannot secure migration backup".into()))?;
        }
    }
    Ok(())
}

pub(crate) fn sync_legacy_key(conn: &Connection, channel_id: i64, secret: &str) -> Result<()> {
    let first: Option<(i64, String)> = conn
        .query_row(
            "SELECT id, secret FROM channel_keys WHERE channel_id=? ORDER BY id LIMIT 1",
            [channel_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    match first {
        None => {
            conn.execute(
                "INSERT INTO channel_keys(channel_id,name,secret) VALUES (?,'Key 1',?)",
                params![channel_id, secret],
            )?;
        }
        Some((id, previous)) if previous != secret => {
            // Legacy edits are only unambiguous for a single-key channel.
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM channel_keys WHERE channel_id=?",
                [channel_id],
                |r| r.get(0),
            )?;
            if count > 1 {
                return Err(StoreError::Invalid(
                    "请在 Key 管理中替换多 Key 渠道的密钥".into(),
                ));
            }
            conn.execute("UPDATE channel_keys SET secret=?,auth_invalid=0,rate_limit_count=0,cooldown_until=NULL,last_error='',generation=generation+1 WHERE id=?", params![secret,id])?;
        }
        _ => {}
    }
    Ok(())
}

impl Store {
    pub(crate) fn initialize_keys(&self) -> Result<()> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS channel_keys(
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                channel_id INTEGER NOT NULL REFERENCES desktop_channels(id) ON DELETE CASCADE,
                name TEXT NOT NULL,
                secret TEXT NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1,
                position INTEGER NOT NULL DEFAULT 0,
                auth_invalid INTEGER NOT NULL DEFAULT 0,
                rate_limit_count INTEGER NOT NULL DEFAULT 0,
                cooldown_until TEXT,
                last_error TEXT NOT NULL DEFAULT '',
                last_used_at TEXT,
                last_success_at TEXT,
                attempts INTEGER NOT NULL DEFAULT 0,
                successes INTEGER NOT NULL DEFAULT 0,
                rate_limits INTEGER NOT NULL DEFAULT 0,
                prompt_tokens INTEGER NOT NULL DEFAULT 0,
                completion_tokens INTEGER NOT NULL DEFAULT 0,
                generation INTEGER NOT NULL DEFAULT 0,
                UNIQUE(channel_id,secret)
            );
            CREATE TABLE IF NOT EXISTS channel_key_models(
                key_id INTEGER NOT NULL REFERENCES channel_keys(id) ON DELETE CASCADE,
                model TEXT NOT NULL,
                unavailable_until TEXT NOT NULL,
                rate_limit_count INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(key_id,model)
            );
            INSERT INTO channel_keys(channel_id,name,secret,cooldown_until,last_error)
                SELECT id,'Key 1',api_key,cooldown_until,last_error FROM desktop_channels
                WHERE id NOT IN (SELECT channel_id FROM channel_keys);",
        )?;
        if let Err(err) = tx.execute(
            "ALTER TABLE channel_key_models ADD COLUMN rate_limit_count INTEGER NOT NULL DEFAULT 0",
            [],
        ) {
            if !err.to_string().to_lowercase().contains("duplicate column") {
                return Err(StoreError::Db(err));
            }
        }
        // Older releases mirrored model-specific 429s into the global Key
        // cooldown. Remove only that recognizable legacy state so an upgrade
        // immediately restores access to the Key's other models.
        tx.execute(
            "UPDATE channel_keys SET cooldown_until=NULL,rate_limit_count=0 \
             WHERE last_error='429：限流冷却' \
             AND EXISTS(SELECT 1 FROM channel_key_models WHERE key_id=channel_keys.id)",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn list_keys(&self, channel_id: i64) -> Result<Vec<ChannelKey>> {
        let conn = self.lock();
        let mut stmt = conn.prepare("SELECT id,channel_id,name,secret,enabled,position,auth_invalid,rate_limit_count,cooldown_until,last_error,last_used_at,last_success_at,attempts,successes,rate_limits,prompt_tokens,completion_tokens,generation FROM channel_keys WHERE channel_id=? ORDER BY position,id")?;
        let rows = stmt.query_map([channel_id], |r| {
            let date = |index| -> rusqlite::Result<Option<DateTime<Utc>>> {
                let text: Option<String> = r.get(index)?;
                Ok(text.and_then(|s| s.parse().ok()))
            };
            Ok(ChannelKey {
                id: r.get(0)?,
                channel_id: r.get(1)?,
                name: r.get(2)?,
                secret: r.get(3)?,
                enabled: r.get(4)?,
                position: r.get(5)?,
                auth_invalid: r.get(6)?,
                rate_limit_count: r.get(7)?,
                cooldown_until: date(8)?,
                last_error: r.get(9)?,
                last_used_at: date(10)?,
                last_success_at: date(11)?,
                attempts: r.get(12)?,
                successes: r.get(13)?,
                rate_limits: r.get(14)?,
                prompt_tokens: r.get(15)?,
                completion_tokens: r.get(16)?,
                generation: r.get(17)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn add_key(&self, channel_id: i64, input: &KeyInput) -> Result<()> {
        let _guard = self.runtime_mu.lock().unwrap_or_else(|p| p.into_inner());
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM desktop_channels WHERE id=?)",
            [channel_id],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(StoreError::NotFound(channel_id));
        }
        let secret = input.secret.trim();
        if secret.is_empty()
            || secret.contains("***")
            || secret.contains('•')
            || secret.contains(['\r', '\n'])
        {
            return Err(StoreError::Invalid("请输入有效的完整 Key".into()));
        }
        let name = if input.name.trim().is_empty() {
            "新 Key"
        } else {
            input.name.trim()
        };
        let duplicate: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM channel_keys WHERE channel_id=? AND secret=?)",
            params![channel_id, secret],
            |row| row.get(0),
        )?;
        if duplicate {
            return Err(StoreError::Invalid("该 Key 已存在".into()));
        }
        tx.execute("INSERT INTO channel_keys(channel_id,name,secret,position) VALUES (?,?,?,COALESCE((SELECT MAX(position)+1 FROM channel_keys WHERE channel_id=?),0))",params![channel_id,name,secret,channel_id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn patch_key(&self, channel_id: i64, id: i64, patch: &KeyPatch) -> Result<()> {
        let _guard = self.runtime_mu.lock().unwrap_or_else(|p| p.into_inner());
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let first: Option<i64> = tx
            .query_row(
                "SELECT id FROM channel_keys WHERE channel_id=? ORDER BY id LIMIT 1",
                [channel_id],
                |r| r.get(0),
            )
            .optional()?;
        let found: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM channel_keys WHERE channel_id=? AND id=?)",
            params![channel_id, id],
            |r| r.get(0),
        )?;
        if !found {
            return Err(StoreError::NotFound(id));
        }
        if let Some(secret) = &patch.secret {
            let secret = secret.trim();
            if secret.is_empty()
                || secret.contains("***")
                || secret.contains('•')
                || secret.contains(['\r', '\n'])
            {
                return Err(StoreError::Invalid("请输入有效的完整 Key".into()));
            }
            tx.execute("UPDATE channel_keys SET secret=?,auth_invalid=0,rate_limit_count=0,cooldown_until=NULL,last_error='',generation=generation+1 WHERE id=?",params![secret,id])?;
            tx.execute("DELETE FROM channel_key_models WHERE key_id=?", [id])?;
            if first == Some(id) {
                tx.execute(
                    "UPDATE desktop_channels SET api_key=? WHERE id=?",
                    params![secret, channel_id],
                )?;
            }
        }
        if let Some(name) = &patch.name {
            if name.trim().is_empty() {
                return Err(StoreError::Invalid("Key 名称不能为空".into()));
            }
            tx.execute(
                "UPDATE channel_keys SET name=? WHERE id=?",
                params![name.trim(), id],
            )?;
        }
        if let Some(enabled) = patch.enabled {
            // Enabling is not a cooldown bypass.
            tx.execute(
                "UPDATE channel_keys SET enabled=? WHERE id=?",
                params![enabled, id],
            )?;
        }
        if let Some(position) = patch.position {
            tx.execute(
                "UPDATE channel_keys SET position=? WHERE id=?",
                params![position, id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn delete_key(&self, channel_id: i64, id: i64) -> Result<()> {
        let _guard = self.runtime_mu.lock().unwrap_or_else(|p| p.into_inner());
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM channel_keys WHERE channel_id=?",
            [channel_id],
            |r| r.get(0),
        )?;
        if count <= 1 {
            return Err(StoreError::Invalid(
                "请保留至少一条凭证；可停用最后一把 Key".into(),
            ));
        }
        if tx.execute(
            "DELETE FROM channel_keys WHERE channel_id=? AND id=?",
            params![channel_id, id],
        )? == 0
        {
            return Err(StoreError::NotFound(id));
        }
        tx.execute("UPDATE desktop_channels SET api_key=(SELECT secret FROM channel_keys WHERE channel_id=? ORDER BY id LIMIT 1) WHERE id=?",params![channel_id,channel_id])?;
        tx.commit()?;
        Ok(())
    }
}
