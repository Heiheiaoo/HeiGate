//! Portable configuration contains no runtime state. Imports append atomically.
use crate::{
    probe,
    store::{Result, Store, StoreError},
};
use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct BackupKey {
    pub name: String,
    pub secret: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub position: i64,
}
fn yes() -> bool {
    true
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupChannel {
    pub name: String,
    pub url: String,
    pub protocol: String,
    pub primary_model: String,
    #[serde(default)]
    pub model_names: Vec<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub keys: Vec<BackupKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Backup {
    pub format: String,
    pub version: u32,
    #[serde(default)]
    pub includes_secrets: Option<bool>,
    pub channels: Vec<BackupChannel>,
}

fn validate(backup: &Backup) -> Result<()> {
    if backup.format != "heigate-config"
        || ![1, 2].contains(&backup.version)
        || backup.includes_secrets == Some(false)
        || backup.channels.is_empty()
        || backup.channels.len() > 100
    {
        return Err(StoreError::Invalid(
            "请选择含完整密钥的 v1/v2 配置（1–100 个渠道）；脱敏配置不能直接恢复".into(),
        ));
    }
    for channel in &backup.channels {
        let keys = channel_keys(channel);
        if channel.name.trim().is_empty()
            || channel.url.trim().is_empty()
            || channel.primary_model.trim().is_empty()
            || keys.is_empty()
        {
            return Err(StoreError::Invalid(
                "渠道名称、地址、主模型和 Key 列表必须完整".into(),
            ));
        }
        if !["anthropic", "openai-compatible", "openai-responses"]
            .contains(&channel.protocol.as_str())
        {
            return Err(StoreError::Invalid("不支持的渠道协议".into()));
        }
        for key in keys {
            let secret = key
                .secret
                .as_deref()
                .ok_or_else(|| StoreError::Invalid("配置缺少完整 Key".into()))?;
            if secret.trim().is_empty()
                || secret.contains("***")
                || secret.contains('•')
                || secret.contains(['\n', '\r'])
            {
                return Err(StoreError::Invalid("配置包含遮罩或无效 Key".into()));
            }
        }
        let (provider, api_mode) = protocol(&channel.protocol);
        probe::validate_config(&probe::Config {
            provider: provider.into(),
            api_mode: api_mode.into(),
            endpoint: channel.url.clone(),
            api_key: String::new(),
            primary_model: channel.primary_model.clone(),
            ..Default::default()
        })
        .map_err(|_| StoreError::Invalid("渠道地址或模型配置无效".into()))?;
    }
    Ok(())
}

fn channel_keys(channel: &BackupChannel) -> Vec<BackupKey> {
    if !channel.keys.is_empty() {
        channel.keys.clone()
    } else {
        vec![BackupKey {
            name: "Key 1".into(),
            secret: channel.api_key.clone(),
            enabled: true,
            position: 0,
        }]
    }
}

fn protocol(value: &str) -> (&str, &str) {
    match value {
        "anthropic" => ("anthropic", "chat_completions"),
        "openai-responses" => ("openai", "responses"),
        _ => ("openai", "chat_completions"),
    }
}

fn insert_channel(conn: &Connection, channel: &BackupChannel) -> Result<i64> {
    let keys = channel_keys(channel);
    let (provider, mode) = protocol(&channel.protocol);
    let now = Utc::now().to_rfc3339();
    conn.execute("INSERT INTO desktop_channels(name,provider,api_mode,endpoint,api_key,primary_model,extra_models_json,enabled,interval_seconds,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,0,?,?)",
        params![channel.name.trim(),provider,mode,channel.url.trim().trim_end_matches('/'),keys[0].secret.as_deref().unwrap_or_default().trim(),channel.primary_model.trim(),serde_json::to_string(&channel.model_names)?,channel.enabled,now,now])?;
    let id = conn.last_insert_rowid();
    for key in keys {
        conn.execute("INSERT OR IGNORE INTO channel_keys(channel_id,name,secret,enabled,position) VALUES (?,?,?,?,?)",params![id,key.name,key.secret.as_deref().unwrap_or_default().trim(),key.enabled,key.position])?;
    }
    Ok(id)
}

impl Store {
    pub fn duplicate_with_keys(&self, id: i64) -> Result<i64> {
        let _guard = self.runtime_mu.lock().unwrap_or_else(|p| p.into_inner());
        let channel = self.get(id)?;
        let keys = self.list_keys(id)?;
        let config = BackupChannel {
            name: format!("{} (副本)", channel.name),
            url: channel.endpoint,
            protocol: if channel.provider == "anthropic" {
                "anthropic"
            } else if channel.api_mode == "responses" {
                "openai-responses"
            } else {
                "openai-compatible"
            }
            .into(),
            primary_model: channel.primary_model,
            model_names: channel.extra_models,
            enabled: channel.enabled,
            keys: keys
                .iter()
                .map(|k| BackupKey {
                    name: k.name.clone(),
                    secret: Some(k.secret.clone()),
                    enabled: k.enabled,
                    position: k.position,
                })
                .collect(),
            api_key: None,
        };
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let new_id = insert_channel(&tx, &config)?;
        for key in keys {
            tx.execute("UPDATE channel_keys SET auth_invalid=?,cooldown_until=?,rate_limit_count=?,last_error=?,generation=? WHERE channel_id=? AND secret=?",params![key.auth_invalid,key.cooldown_until.map(|t|t.to_rfc3339()),key.rate_limit_count,key.last_error,key.generation,new_id,key.secret])?;
            tx.execute("INSERT INTO channel_key_models(key_id,model,unavailable_until) SELECT target.id,m.model,m.unavailable_until FROM channel_key_models m JOIN channel_keys target ON target.channel_id=? AND target.secret=? WHERE m.key_id=?", params![new_id,key.secret,key.id])?;
        }
        tx.commit()?;
        Ok(new_id)
    }

    pub fn export_config(&self, secrets: bool) -> Result<Backup> {
        let _guard = self.runtime_mu.lock().unwrap_or_else(|p| p.into_inner());
        let mut channels = Vec::new();
        for channel in self.list()? {
            let keys = self
                .list_keys(channel.id)?
                .into_iter()
                .map(|key| BackupKey {
                    name: key.name,
                    secret: secrets.then_some(key.secret),
                    enabled: key.enabled,
                    position: key.position,
                })
                .collect();
            channels.push(BackupChannel {
                name: channel.name,
                url: channel.endpoint,
                protocol: if channel.provider == "anthropic" {
                    "anthropic"
                } else if channel.api_mode == "responses" {
                    "openai-responses"
                } else {
                    "openai-compatible"
                }
                .into(),
                primary_model: channel.primary_model,
                model_names: channel.extra_models,
                enabled: channel.enabled,
                keys,
                api_key: None,
            });
        }
        Ok(Backup {
            format: "heigate-config".into(),
            version: 2,
            includes_secrets: Some(secrets),
            channels,
        })
    }

    pub fn preview_import(&self, backup: &Backup) -> Result<serde_json::Value> {
        validate(backup)?;
        Ok(
            serde_json::json!({"channels":backup.channels.len(),"keys":backup.channels.iter().map(|c|channel_keys(c).len()).sum::<usize>(),"names":backup.channels.iter().map(|c|&c.name).collect::<Vec<_>>(),"mode":"append"}),
        )
    }

    pub fn import_config(&self, backup: &Backup) -> Result<Vec<i64>> {
        validate(backup)?;
        let _guard = self.runtime_mu.lock().unwrap_or_else(|p| p.into_inner());
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let ids = backup
            .channels
            .iter()
            .map(|channel| insert_channel(&tx, channel))
            .collect::<Result<Vec<_>>>()?;
        tx.commit()?;
        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> Backup {
        serde_json::from_value(serde_json::json!({"format":"heigate-config","version":1,"channels":[{"name":"A","url":"https://example.test","protocol":"openai-compatible","primaryModel":"gpt-4o","apiKey":"test-key"}]})).unwrap()
    }
    #[test]
    fn legacy_roundtrip_and_redacted_import_rejected() {
        let store = Store::open_in_memory().unwrap();
        let ids = store.import_config(&sample()).unwrap();
        assert_eq!(ids.len(), 1);
        let backup = store.export_config(true).unwrap();
        assert_eq!(
            backup.channels[0].keys[0].secret.as_deref(),
            Some("test-key")
        );
        assert!(store
            .import_config(&store.export_config(false).unwrap())
            .is_err());
        let copy = Store::open_in_memory().unwrap();
        copy.import_config(&backup).unwrap();
        assert_eq!(copy.list().unwrap().len(), 1);
    }
    #[test]
    fn invalid_import_is_atomic() {
        let store = Store::open_in_memory().unwrap();
        let mut backup = sample();
        let mut invalid = backup.channels[0].clone();
        invalid.api_key = Some("***".into());
        backup.channels.push(invalid);
        assert!(store.import_config(&backup).is_err());
        assert!(store.list().unwrap().is_empty());
    }
}
