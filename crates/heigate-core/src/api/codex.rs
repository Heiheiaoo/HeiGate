//! Safe local integration with the OpenAI Codex client.
//!
//! Codex keeps its provider configuration in `config.toml`.  This module only
//! owns the HeiGate provider entry and the top-level model/provider selection;
//! all unrelated TOML items are retained by `toml_edit`.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use toml_edit::{value, DocumentMut, Item, Table};

#[derive(Debug, Clone, Serialize)]
pub struct CodexConfigStatus {
    pub path: String,
    pub exists: bool,
    pub managed: bool,
    pub modified: bool,
    pub backup_exists: bool,
    pub parse_error: Option<String>,
    pub provider: Option<String>,
    pub catalog_path: String,
    pub catalog_managed: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CodexApplyOptions {
    pub model: String,
    pub gateway_base_url: String,
    pub gateway_key: String,
    pub mode: String,
    pub direct_base_url: Option<String>,
    pub direct_key: Option<String>,
    pub direct_models: Vec<String>,
}

fn codex_home() -> PathBuf {
    if let Ok(path) = std::env::var("CODEX_HOME") {
        let path = path.trim();
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".codex")
}

pub fn config_path() -> PathBuf {
    codex_home().join("config.toml")
}

pub fn catalog_path() -> PathBuf {
    codex_home().join("heigate-models.json")
}

fn backup_path(path: &Path) -> PathBuf {
    path.with_file_name("config.toml.heigate.bak")
}

fn catalog_backup_path(path: &Path) -> PathBuf {
    path.with_file_name("heigate-models.json.heigate.bak")
}

fn atomic_write(path: &Path, content: &str) -> std::io::Result<()> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config");
    let tmp = path.with_file_name(format!(".{file_name}.heigate.tmp"));
    std::fs::write(&tmp, content)?;
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // config.toml contains the gateway bearer token.
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

fn atomic_remove(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn catalog_is_managed(path: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    value.get("managed_by").and_then(serde_json::Value::as_str) == Some("heigate")
}

fn managed(doc: &DocumentMut) -> bool {
    let provider = doc.get("model_provider").and_then(Item::as_str);
    let provider_id = match provider {
        Some("heigate") | Some("heigate_direct") => provider.unwrap(),
        _ => return false,
    };
    let item = doc
        .get("model_providers")
        .and_then(Item::as_table)
        .and_then(|table| table.get(provider_id))
        .and_then(Item::as_table);
    provider.is_some_and(|id| id == "heigate" || id == "heigate_direct")
        && item.is_some_and(|table| {
            table.get("base_url").and_then(Item::as_str).is_some()
                && table.get("wire_api").and_then(Item::as_str) == Some("responses")
        })
}

pub fn status() -> CodexConfigStatus {
    let path = config_path();
    let backup = backup_path(&path);
    let catalog = catalog_path();
    let exists = path.exists();
    if !exists {
        return CodexConfigStatus {
            path: path.display().to_string(),
            exists: false,
            managed: false,
            modified: false,
            backup_exists: backup.exists(),
            parse_error: None,
            provider: None,
            catalog_path: catalog.display().to_string(),
            catalog_managed: catalog_is_managed(&catalog),
        };
    }
    match std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| raw.parse::<DocumentMut>().ok())
    {
        Some(doc) => {
            let managed = managed(&doc);
            CodexConfigStatus {
                path: path.display().to_string(),
                exists: true,
                managed,
                modified: !managed,
                backup_exists: backup.exists(),
                parse_error: None,
                provider: doc
                    .get("model_provider")
                    .and_then(Item::as_str)
                    .map(str::to_string),
                catalog_path: catalog.display().to_string(),
                catalog_managed: catalog_is_managed(&catalog),
            }
        }
        None => CodexConfigStatus {
            path: path.display().to_string(),
            exists: true,
            managed: false,
            modified: false,
            backup_exists: backup.exists(),
            parse_error: Some("config.toml 不是有效的 TOML，已拒绝覆盖".into()),
            provider: None,
            catalog_path: catalog.display().to_string(),
            catalog_managed: catalog_is_managed(&catalog),
        },
    }
}

pub fn apply(options: &CodexApplyOptions) -> Result<CodexConfigStatus, String> {
    let path = config_path();
    let backup = backup_path(&path);
    let catalog = catalog_path();
    let catalog_backup = catalog_backup_path(&catalog);
    let original = if path.exists() {
        Some(std::fs::read_to_string(&path).map_err(|e| format!("读取 Codex 配置失败: {e}"))?)
    } else {
        None
    };
    let mut doc = match original.as_deref() {
        Some(raw) => raw
            .parse::<DocumentMut>()
            .map_err(|_| "Codex config.toml 不是有效的 TOML，已拒绝覆盖".to_string())?,
        None => DocumentMut::new(),
    };
    let was_managed = managed(&doc);

    let mode = options.mode.trim();
    let direct = mode == "direct";
    if direct {
        if options
            .direct_base_url
            .as_deref()
            .unwrap_or("")
            .trim()
            .is_empty()
        {
            return Err("直连模式缺少供应商地址".into());
        }
        if options
            .direct_key
            .as_deref()
            .unwrap_or("")
            .trim()
            .is_empty()
        {
            return Err("直连模式缺少供应商 API Key".into());
        }
        if options.direct_models.is_empty() {
            return Err("直连模式至少需要一个模型".into());
        }
    } else if mode != "gateway" {
        return Err("Codex 接入模式无效".into());
    }

    std::fs::create_dir_all(path.parent().unwrap_or_else(|| Path::new(".")))
        .map_err(|e| format!("创建 Codex 配置目录失败: {e}"))?;

    // Snapshot the active user config each time we enter HeiGate from an
    // unmanaged Codex config. An empty snapshot represents Codex defaults when
    // config.toml did not exist before the first takeover.
    if !was_managed {
        atomic_write(&backup, original.as_deref().unwrap_or(""))
            .map_err(|e| format!("保存 Codex 配置备份失败: {e}"))?;

        if catalog.exists() && !catalog_is_managed(&catalog) {
            let raw = std::fs::read_to_string(&catalog)
                .map_err(|e| format!("读取 Codex 模型目录失败: {e}"))?;
            atomic_write(&catalog_backup, &raw)
                .map_err(|e| format!("保存 Codex 模型目录备份失败: {e}"))?;
        } else {
            atomic_remove(&catalog_backup)
                .map_err(|e| format!("清理旧 Codex 模型目录备份失败: {e}"))?;
        }
    }

    let provider_id = if direct { "heigate_direct" } else { "heigate" };
    doc["model"] = value(options.model.trim());
    doc["model_provider"] = value(provider_id);
    if !options.direct_models.is_empty() {
        doc["model_catalog_json"] = value(catalog.display().to_string());
    } else if doc
        .get("model_catalog_json")
        .and_then(Item::as_str)
        .is_some_and(|path| path == catalog.display().to_string())
    {
        doc.remove("model_catalog_json");
    }
    let providers = doc["model_providers"].or_insert(Item::Table(Table::new()));
    let providers = providers
        .as_table_mut()
        .ok_or_else(|| "Codex 配置中的 model_providers 必须是表格".to_string())?;
    let provider = providers
        .entry(provider_id)
        .or_insert(Item::Table(Table::new()));
    let provider = provider
        .as_table_mut()
        .ok_or_else(|| "Codex 配置中的 heigate provider 必须是表格".to_string())?;
    provider["name"] = value(if direct { "HeiGate Direct" } else { "HeiGate" });
    provider["base_url"] = value(if direct {
        normalize_direct_base_url(options.direct_base_url.as_deref().unwrap_or(""))
    } else {
        options.gateway_base_url.trim().to_string()
    });
    provider["wire_api"] = value("responses");
    provider["requires_openai_auth"] = value(false);
    provider["experimental_bearer_token"] = value(if direct {
        options.direct_key.as_deref().unwrap_or("").trim()
    } else {
        options.gateway_key.trim()
    });
    provider["supports_websockets"] = value(false);

    let catalog_content = if !options.direct_models.is_empty() {
        Some(render_catalog(
            &options.direct_models,
            if direct { "direct" } else { "gateway" },
        ))
    } else {
        None
    };
    let config_content = doc.to_string();
    let original_catalog = if catalog.exists() {
        Some(
            std::fs::read_to_string(&catalog)
                .map_err(|e| format!("读取 Codex 模型目录失败: {e}"))?,
        )
    } else {
        None
    };
    atomic_write(&path, &config_content).map_err(|e| format!("写入 Codex 配置失败: {e}"))?;
    let catalog_result = if let Some(content) = catalog_content.as_deref() {
        atomic_write(&catalog, content)
    } else if catalog_is_managed(&catalog) {
        atomic_remove(&catalog)
    } else {
        Ok(())
    };
    if let Err(error) = catalog_result {
        let rollback = match original_catalog {
            Some(original) => atomic_write(&catalog, &original),
            None => atomic_remove(&catalog),
        };
        let config_rollback = if let Some(original) = original.as_deref() {
            atomic_write(&path, original)
        } else {
            atomic_remove(&path)
        };
        let suffix = if rollback.is_err() || config_rollback.is_err() {
            "；回滚失败，请检查 Codex 配置文件"
        } else {
            "；已回滚配置"
        };
        return Err(format!("写入 Codex 模型目录失败: {error}{suffix}"));
    }
    Ok(status())
}

fn normalize_direct_base_url(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed
        .rsplit('/')
        .next()
        .is_some_and(|segment| segment.eq_ignore_ascii_case("v1"))
    {
        trimmed.to_string()
    } else {
        format!("{trimmed}/v1")
    }
}

fn render_catalog(models: &[String], mode: &str) -> String {
    let source = if mode == "gateway" {
        "HeiGate Gateway"
    } else {
        "HeiGate Direct"
    };
    let models = models
        .iter()
        .map(|model| model.trim())
        .filter(|model| !model.is_empty())
        .enumerate()
        .map(|(index, model)| {
            // Keep Codex's required ModelInfo fields present. In particular,
            // several Option fields still require an explicit null, while
            // base_instructions remains accepted by Codex's legacy loader.
            serde_json::json!({
                "slug": model,
                "display_name": model,
                "description": format!("{model} via {source}"),
                "availability_nux": null,
                "upgrade": null,
                "base_instructions": "You are Codex, an agent compatible with the configured model provider.",
                "default_reasoning_level": "medium",
                "supported_reasoning_levels": [],
                "visibility": "list",
                "supported_in_api": true,
                "supports_reasoning_summary_parameter": false,
                "supports_reasoning_summaries": false,
                "supports_parallel_tool_calls": false,
                "input_modalities": ["text"],
                "priority": index + 1,
                "shell_type": "shell_command",
                "support_verbosity": true,
                "default_verbosity": null,
                "apply_patch_tool_type": null,
                "truncation_policy": {
                    "limit": 10000,
                    "mode": "tokens"
                },
                "experimental_supported_tools": []
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({ "managed_by": "heigate", "models": models }).to_string()
}

pub fn restore() -> Result<CodexConfigStatus, String> {
    let path = config_path();
    let backup = backup_path(&path);
    let catalog = catalog_path();
    let catalog_backup = catalog_backup_path(&catalog);
    let config_content = if backup.exists() {
        let raw =
            std::fs::read_to_string(&backup).map_err(|e| format!("读取 Codex 备份失败: {e}"))?;
        raw.parse::<DocumentMut>()
            .map_err(|_| "Codex 备份不是有效的 TOML，已拒绝恢复".to_string())?;
        Some(raw)
    } else if !path.exists() && catalog_is_managed(&catalog) {
        // The config may have been removed manually after an older HeiGate
        // release took over. Removing the orphaned managed catalog restores
        // Codex's built-in defaults.
        None
    } else {
        // Older HeiGate versions did not create a backup when config.toml was
        // absent. In that case, remove only the settings owned by this
        // integration and let Codex return to its built-in provider defaults.
        let raw =
            std::fs::read_to_string(&path).map_err(|e| format!("读取当前 Codex 配置失败: {e}"))?;
        let mut doc = raw
            .parse::<DocumentMut>()
            .map_err(|_| "没有接管前备份，且当前 Codex 配置无法解析，无法安全切回".to_string())?;
        if !managed(&doc) {
            return Err("没有接管前的 Codex 配置备份，无法恢复官方配置".into());
        }
        let provider_id = doc
            .get("model_provider")
            .and_then(Item::as_str)
            .unwrap_or_default()
            .to_string();
        doc.remove("model");
        doc.remove("model_provider");
        if doc
            .get("model_catalog_json")
            .and_then(Item::as_str)
            .is_some_and(|configured| configured == catalog.display().to_string())
        {
            doc.remove("model_catalog_json");
        }
        let providers_empty =
            if let Some(providers) = doc.get_mut("model_providers").and_then(Item::as_table_mut) {
                providers.remove(&provider_id);
                providers.is_empty()
            } else {
                false
            };
        if providers_empty {
            doc.remove("model_providers");
        }
        Some(doc.to_string())
    };
    let catalog_content = if catalog_backup.exists() {
        Some(
            std::fs::read_to_string(&catalog_backup)
                .map_err(|e| format!("读取 Codex 模型目录备份失败: {e}"))?,
        )
    } else {
        None
    };
    let current_catalog = if catalog.exists() {
        Some(
            std::fs::read_to_string(&catalog)
                .map_err(|e| format!("读取当前 Codex 模型目录失败: {e}"))?,
        )
    } else {
        None
    };
    let catalog_result = if let Some(original) = catalog_content.as_deref() {
        atomic_write(&catalog, original)
    } else if catalog_is_managed(&catalog) {
        atomic_remove(&catalog)
    } else {
        Ok(())
    };
    if let Err(error) = catalog_result {
        return Err(format!("恢复 Codex 模型目录失败: {error}"));
    }
    let config_result = match config_content.as_deref() {
        Some("") => atomic_remove(&path),
        Some(content) => atomic_write(&path, content),
        None => atomic_remove(&path),
    };
    if let Err(error) = config_result {
        let catalog_rollback = match current_catalog {
            Some(original) => atomic_write(&catalog, &original),
            None => atomic_remove(&catalog),
        };
        let suffix = if catalog_rollback.is_err() {
            "；模型目录回滚失败，请检查 Codex 配置"
        } else {
            "；模型目录已回滚"
        };
        return Err(format!("恢复 Codex 配置失败: {error}{suffix}"));
    }
    Ok(status())
}

/// Restart the installed Codex desktop host when one is available. The CLI
/// is intentionally not killed: it may be running a user task and reads the
/// updated config on its next invocation.
pub fn restart() -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        for app in ["Codex", "ChatGPT"] {
            let app_path = format!("{app}.app/Contents/MacOS/");
            let running = Command::new("pgrep")
                .args(["-f", &app_path])
                .output()
                .is_ok_and(|out| out.status.success());
            if running {
                let quit = Command::new("osascript")
                    .args(["-e", &format!("tell application \"{app}\" to quit")])
                    .output()
                    .map_err(|e| format!("退出 {app} 失败: {e}"))?;
                if !quit.status.success() {
                    return Err(format!(
                        "退出 {app} 失败: {}",
                        String::from_utf8_lossy(&quit.stderr).trim()
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            let open = Command::new("open")
                .args(["-a", app])
                .output()
                .map_err(|e| format!("启动 {app} 失败: {e}"))?;
            if open.status.success() {
                return Ok(if running {
                    format!("已重启 {app}")
                } else {
                    format!("已启动 {app}")
                });
            }
        }
        Ok("配置已写入；未找到可启动的 Codex 桌面应用，CLI 将在下次启动时读取新配置".into())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok("配置已写入；当前平台未自动终止正在运行的 Codex CLI，下次启动时读取新配置".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_only_a_complete_heigate_responses_provider() {
        let doc = r#"
model = "gpt-4o"
model_provider = "heigate"

[model_providers.heigate]
base_url = "http://127.0.0.1:28888/codex/v1"
wire_api = "responses"
"#
        .parse::<DocumentMut>()
        .unwrap();
        assert!(managed(&doc));

        let chat = r#"
model_provider = "heigate"
[model_providers.heigate]
base_url = "http://127.0.0.1:28888/codex/v1"
wire_api = "chat"
"#
        .parse::<DocumentMut>()
        .unwrap();
        assert!(!managed(&chat));

        let direct = r#"
model_provider = "heigate_direct"
[model_providers.heigate_direct]
base_url = "https://supplier.example/v1"
wire_api = "responses"
"#
        .parse::<DocumentMut>()
        .unwrap();
        assert!(managed(&direct));
    }

    #[test]
    fn renders_a_codex_catalog_with_stable_model_metadata() {
        let raw = render_catalog(&["model-a".into(), "model-b".into()], "direct");
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(value["managed_by"], "heigate");
        assert_eq!(value["models"].as_array().unwrap().len(), 2);
        assert_eq!(value["models"][0]["slug"], "model-a");
        assert_eq!(value["models"][0]["shell_type"], "shell_command");
        assert_eq!(value["models"][1]["priority"], 2);
        assert_eq!(
            value["models"][0]["description"],
            "model-a via HeiGate Direct"
        );
        assert!(value["models"][0]["base_instructions"]
            .as_str()
            .unwrap()
            .contains("configured model provider"));
    }

    #[test]
    fn normalizes_direct_base_urls_for_codex_responses() {
        assert_eq!(
            normalize_direct_base_url("https://supplier.example"),
            "https://supplier.example/v1"
        );
        assert_eq!(
            normalize_direct_base_url("https://supplier.example/"),
            "https://supplier.example/v1"
        );
        assert_eq!(
            normalize_direct_base_url("https://supplier.example/v1/"),
            "https://supplier.example/v1"
        );
        assert_eq!(
            normalize_direct_base_url("https://supplier.example/compatible-mode"),
            "https://supplier.example/compatible-mode/v1"
        );
    }

    #[test]
    fn preserves_unknown_provider_fields_when_editing_document() {
        let mut doc = r#"
custom_setting = true
[model_providers.other]
name = "Other"
"#
        .parse::<DocumentMut>()
        .unwrap();
        doc["model"] = value("o3-mini");
        doc["model_provider"] = value("heigate");
        assert_eq!(doc["custom_setting"].as_bool(), Some(true));
        assert_eq!(
            doc["model_providers"]["other"]["name"].as_str(),
            Some("Other")
        );
    }
}
