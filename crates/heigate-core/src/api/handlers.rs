//! The local management API (`/api/*`). Ported from `internal/api/handler.go`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::api::gateway::{Gateway, RequestContext};
use crate::api::policy::AUTO_MODEL_NAME;
use crate::canonical::{
    are_models_equivalent, get_canonical_group_key, get_custom_model_mappings,
    select_canonical_display_name, set_custom_model_mappings,
};
use crate::probe::{
    self, check_injection_models, validate_config, validate_interval, ProbeError,
    API_MODE_CHAT_COMPLETIONS,
};
use crate::router::{
    Router, ROUTE_LATENCY, ROUTE_MANUAL_PRIORITY, ROUTE_ROUND_ROBIN, ROUTE_SMART_QUALITY,
};
use crate::runner::{ProbeRunner, RunnerError, ERR_CHANNEL_DISABLED, ERR_PROBE_BUSY};
use crate::store::{copy_config_state, Channel, Store, StoreError};

/// Shared application state for the management API and the gateway.
pub struct AppState {
    pub store: Arc<Store>,
    pub runner: Arc<ProbeRunner>,
    pub router: Arc<Router>,
    pub gateway: Arc<Gateway>,
    pub gateway_key: String,
    pub gateway_port: u16,
    pub version: String,
}

pub type SharedState = Arc<AppState>;

const MAX_UPSTREAM_RESPONSE_BODY: usize = 2 << 20;

async fn read_upstream_body(response: reqwest::Response) -> Result<Vec<u8>, ()> {
    crate::probe::read_response_limited(response, MAX_UPSTREAM_RESPONSE_BODY)
        .await
        .map_err(|_| ())
}

fn ok_data(data: Value) -> Response {
    json_response(StatusCode::OK, json!({ "data": data }))
}

fn json_response(status: StatusCode, body: Value) -> Response {
    let mut response = Response::new(Body::from(body.to_string()));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert("content-type", HeaderValue::from_static("application/json"));
    response.headers_mut().insert(
        "cache-control",
        HeaderValue::from_static("no-store, private"),
    );
    response
}

/// Maps a store error to an HTTP status the way Go's typed-sentinel `writeError`
/// does.
fn store_error_response(err: &StoreError) -> Response {
    let status = match err {
        StoreError::NotFound(_) => StatusCode::NOT_FOUND,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    json_response(status, json!({ "error": err.to_string() }))
}

fn probe_error_response(err: &ProbeError) -> Response {
    json_response(StatusCode::BAD_REQUEST, json!({ "error": err.to_string() }))
}

fn runner_error_response(err: &RunnerError) -> Response {
    match err {
        RunnerError::Busy => json_response(
            StatusCode::CONFLICT,
            json!({ "code": "probe_busy", "error": ERR_PROBE_BUSY }),
        ),
        RunnerError::Disabled => json_response(
            StatusCode::CONFLICT,
            json!({ "code": "channel_disabled", "error": ERR_CHANNEL_DISABLED }),
        ),
        RunnerError::NotFound(_) => {
            json_response(StatusCode::NOT_FOUND, json!({ "error": err.to_string() }))
        }
        RunnerError::Store(_) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": err.to_string() }),
        ),
    }
}

fn include_secrets(params: &HashMap<String, String>) -> bool {
    matches!(
        params.get("include_secrets").map(String::as_str),
        Some("1") | Some("true")
    )
}

/// Masks the API key to its first four characters unless secrets are requested.
pub fn sanitize_channel(ch: &Channel) -> Channel {
    let mut copy = ch.clone();
    copy.api_key = if copy.api_key.chars().count() > 4 {
        format!("{}***", copy.api_key.chars().take(4).collect::<String>())
    } else {
        "***".to_string()
    };
    copy
}

pub async fn session(State(state): State<SharedState>) -> Response {
    ok_data(json!({ "api_key": state.gateway_key }))
}

pub async fn list_channels(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    match state.store.list() {
        Ok(channels) => {
            let channels = if include_secrets(&params) {
                channels
            } else {
                channels.iter().map(sanitize_channel).collect()
            };
            let data: Vec<Value> = channels
                .iter()
                .map(|channel| {
                    let mut value = json!(channel);
                    if let Ok(keys) = state.store.list_keys(channel.id) {
                        let now = chrono::Utc::now();
                        value["key_count"] = json!(keys.len());
                        value["key_available_count"] = json!(keys
                            .iter()
                            .filter(|k| k.enabled
                                && !k.auth_invalid
                                && !k.cooldown_until.is_some_and(|t| t > now))
                            .count());
                        value["key_cooling_count"] = json!(keys
                            .iter()
                            .filter(|k| k.enabled && k.cooldown_until.is_some_and(|t| t > now))
                            .count());
                    }
                    value
                })
                .collect();
            ok_data(json!(data))
        }
        Err(err) => store_error_response(&err),
    }
}

pub async fn get_channel(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    match state.store.get(id) {
        Ok(channel) => {
            let channel = if include_secrets(&params) {
                channel
            } else {
                sanitize_channel(&channel)
            };
            ok_data(json!(channel))
        }
        Err(err) => store_error_response(&err),
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct ChannelRequest {
    #[serde(default)]
    name: String,
    #[serde(default)]
    provider: String,
    #[serde(default)]
    api_mode: String,
    #[serde(default)]
    endpoint: String,
    #[serde(default)]
    api_key: String,
    #[serde(default)]
    primary_model: String,
    #[serde(default)]
    extra_models: Vec<String>,
    #[serde(default)]
    enabled: Option<bool>,
}

/// Builds a channel from user input, defaulting the API mode and endpoint and
/// validating the configuration.
fn request_to_channel(input: ChannelRequest) -> Result<Channel, ProbeError> {
    let enabled = input.enabled.unwrap_or(true);
    let api_mode = if input.api_mode.is_empty() {
        API_MODE_CHAT_COMPLETIONS.to_string()
    } else {
        input.api_mode
    };
    let mut endpoint = input.endpoint.trim().trim_end_matches('/').to_string();
    let lower = endpoint.to_lowercase();
    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        endpoint = format!("https://{endpoint}");
    }

    let channel = Channel {
        name: input.name,
        provider: input.provider,
        api_mode,
        endpoint,
        api_key: input.api_key,
        primary_model: input.primary_model,
        extra_models: input.extra_models,
        enabled,
        // Automatic probes are disabled globally; keep the persisted value at
        // zero so old clients cannot re-enable quota-consuming probes.
        interval_seconds: 0,
        ..Default::default()
    };
    validate_interval(channel.interval_seconds)?;
    validate_config(&probe::Config {
        provider: channel.provider.clone(),
        api_mode: channel.api_mode.clone(),
        endpoint: channel.endpoint.clone(),
        api_key: channel.api_key.clone(),
        primary_model: channel.primary_model.clone(),
        ..Default::default()
    })?;
    Ok(channel)
}

pub async fn create_channel(
    State(state): State<SharedState>,
    Json(input): Json<ChannelRequest>,
) -> Response {
    let mut channel = match request_to_channel(input) {
        Ok(channel) => channel,
        Err(err) => return probe_error_response(&err),
    };
    match state.store.save(&mut channel) {
        Ok(()) => json_response(
            StatusCode::CREATED,
            json!({ "data": sanitize_channel(&channel) }),
        ),
        Err(err) => store_error_response(&err),
    }
}

pub async fn update_channel(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Json(input): Json<ChannelRequest>,
) -> Response {
    let existing = match state.store.get(id) {
        Ok(existing) => existing,
        Err(err) => return store_error_response(&err),
    };

    // A blank or masked key means "keep the stored one".
    let mut input = input;
    let keep_key = input.api_key.trim().is_empty() || input.api_key.contains("***");
    if keep_key {
        input.api_key = existing.api_key.clone();
    }

    let mut incoming = match request_to_channel(input) {
        Ok(channel) => channel,
        Err(err) => return probe_error_response(&err),
    };

    let result = state.store.update_runtime(id, |fresh| {
        if keep_key {
            incoming.api_key.clone_from(&fresh.api_key);
        }
        if fresh.interval_seconds != incoming.interval_seconds
            || fresh.primary_model != incoming.primary_model
        {
            fresh.probe_streak = 0;
        }
        copy_config_state(fresh, &incoming);
    });
    match result {
        Ok(()) => {
            let updated = state.store.get(id).unwrap_or(existing);
            ok_data(json!(sanitize_channel(&updated)))
        }
        Err(err) => store_error_response(&err),
    }
}

pub async fn delete_channel(State(state): State<SharedState>, Path(id): Path<i64>) -> Response {
    match state.store.delete(id) {
        Ok(()) => {
            let mut response = Response::new(Body::empty());
            *response.status_mut() = StatusCode::NO_CONTENT;
            response
        }
        Err(err) => store_error_response(&err),
    }
}

pub async fn duplicate_channel(State(state): State<SharedState>, Path(id): Path<i64>) -> Response {
    match state
        .store
        .duplicate_with_keys(id)
        .and_then(|new_id| state.store.get(new_id))
    {
        Ok(channel) => json_response(
            StatusCode::CREATED,
            json!({"data":sanitize_channel(&channel)}),
        ),
        Err(err) => store_error_response(&err),
    }
}

pub async fn probe_client(Json(mut cfg): Json<probe::Config>) -> Response {
    cfg.extra_models.clear();
    match probe::run_once(&cfg).await {
        Ok(mut results) => {
            for result in &mut results {
                if !cfg.api_key.is_empty() {
                    result.message = result.message.replace(&cfg.api_key, "[REDACTED]");
                }
            }
            ok_data(json!(results))
        }
        Err(err) => probe_error_response(&err),
    }
}

pub async fn probe_channel(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let target_model = params
        .get("model")
        .map(|m| m.trim().to_string())
        .unwrap_or_default();
    let check_all = matches!(
        params.get("all").map(String::as_str),
        Some("1") | Some("true")
    );

    let result = if !target_model.is_empty() {
        state.runner.probe_model_now(id, &target_model).await
    } else if check_all {
        state.runner.probe_all_models_now(id).await
    } else {
        state.runner.probe_now(id).await
    };

    match result {
        Ok(results) => ok_data(json!(results)),
        Err(err) => runner_error_response(&err),
    }
}

pub async fn injection_check(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let channel = match state.store.get(id) {
        Ok(channel) => channel,
        Err(err) => return store_error_response(&err),
    };
    if !state.runner.acquire_channel(id) {
        return json_response(
            StatusCode::CONFLICT,
            json!({ "code": "probe_busy", "error": ERR_PROBE_BUSY }),
        );
    }
    let _guard = ChannelSlotGuard {
        runner: state.runner.clone(),
        id,
    };

    let target_model = params
        .get("model")
        .map(|m| m.trim().to_string())
        .unwrap_or_default();
    let check_all = matches!(
        params.get("all").map(String::as_str),
        Some("1") | Some("true")
    );

    let models: Vec<String> = if !target_model.is_empty() {
        vec![target_model]
    } else if check_all {
        let mut models = vec![channel.primary_model.clone()];
        models.extend(channel.extra_models.clone());
        models
    } else {
        vec![channel.primary_model.clone()]
    };

    if !channel.enabled {
        return json_response(StatusCode::CONFLICT, json!({"error":"渠道已停用"}));
    }
    let mut reports = Vec::new();
    let mut key_origins = Vec::new();
    for model in &models {
        let lease = match state
            .gateway
            .key_pool
            .acquire_probe_for_channel(&channel, model, None)
        {
            Ok(Some(lease)) => lease,
            _ => {
                return json_response(
                    StatusCode::CONFLICT,
                    json!({"error":"没有可用于检测的 Key（未配置或已禁用）"}),
                )
            }
        };
        let mut result = check_injection_models(
            &probe::Config {
                provider: channel.provider.clone(),
                api_mode: channel.api_mode.clone(),
                endpoint: channel.endpoint.clone(),
                api_key: lease.key.secret.clone(),
                primary_model: model.clone(),
                ..Default::default()
            },
            std::slice::from_ref(model),
        )
        .await;
        for report in &mut result {
            if !lease.key.secret.is_empty() {
                report.message = report.message.replace(&lease.key.secret, "[REDACTED]");
                report.content = report.content.replace(&lease.key.secret, "[REDACTED]");
            }
            let accepted = if report.model == channel.primary_model {
                match state.store.set_injection(
                    &channel,
                    &lease,
                    report.checked_at,
                    &report.state,
                    report.prompt_tokens,
                    &report.content,
                ) {
                    Ok(accepted) => accepted,
                    Err(err) => return store_error_response(&err),
                }
            } else {
                match state
                    .store
                    .diagnostic_is_current(&channel, &report.model, &lease)
                {
                    Ok(accepted) => accepted,
                    Err(err) => return store_error_response(&err),
                }
            };
            key_origins.push((lease.key.id, lease.key.name.clone(), accepted));
        }
        reports.extend(result);
    }

    let reports: Vec<_> = reports
        .iter()
        .zip(key_origins)
        .map(|(report, (id, name, accepted))| {
            let mut value = json!(report);
            value["key_id"] = json!(id);
            value["key_name"] = json!(name);
            value["accepted"] = json!(accepted);
            value
        })
        .collect();
    ok_data(json!({ "results": reports }))
}

struct ChannelSlotGuard {
    runner: Arc<ProbeRunner>,
    id: i64,
}

impl Drop for ChannelSlotGuard {
    fn drop(&mut self) {
        self.runner.release_channel(self.id);
    }
}

pub async fn list_models(State(state): State<SharedState>) -> Response {
    let channels = match state.store.list() {
        Ok(channels) => channels,
        Err(err) => return store_error_response(&err),
    };

    let mut entries: Vec<(String, Vec<String>)> = Vec::new();
    for ch in &channels {
        if !ch.enabled {
            continue;
        }
        let mut models = vec![ch.primary_model.clone()];
        models.extend(ch.extra_models.clone());
        for model in models {
            let model = model.trim().to_string();
            if model.is_empty() {
                continue;
            }
            match entries.iter_mut().find(|(name, _)| *name == model) {
                Some((_, list)) => list.push(ch.name.clone()),
                None => entries.push((model, vec![ch.name.clone()])),
            }
        }
    }

    let mut data: Vec<Value> = Vec::new();
    for (name, channels) in entries {
        data.push(json!({ "name": name, "channels": channels }));
    }
    data.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap_or("")
            .cmp(b["name"].as_str().unwrap_or(""))
    });

    ok_data(json!(data))
}

pub async fn get_custom_mappings() -> Response {
    ok_data(json!(get_custom_model_mappings()))
}

#[derive(Debug, Deserialize, Default)]
pub struct MappingRequest {
    #[serde(default)]
    raw_model: String,
    #[serde(default)]
    target_model: String,
}

pub async fn set_custom_mapping(
    State(state): State<SharedState>,
    Json(input): Json<MappingRequest>,
) -> Response {
    let raw = input.raw_model.trim().to_string();
    let target = input.target_model.trim().to_string();
    if raw.is_empty() || target.is_empty() {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "模型名称不能为空" }),
        );
    }
    let mut mappings = get_custom_model_mappings();
    mappings.insert(raw.to_lowercase(), target);
    let serialized = serde_json::to_string(&mappings).unwrap_or_default();
    if let Err(err) = state
        .store
        .set_setting("custom_model_mappings", &serialized)
    {
        return store_error_response(&err);
    }
    set_custom_model_mappings(mappings.clone());
    json_response(StatusCode::OK, json!({ "status": "ok", "data": mappings }))
}

pub async fn delete_custom_mapping(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let raw = params
        .get("raw_model")
        .map(|m| m.trim().to_string())
        .unwrap_or_default();
    if raw.is_empty() {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "缺少 raw_model 参数" }),
        );
    }
    let mut mappings = get_custom_model_mappings();
    mappings.remove(&raw.to_lowercase());
    let serialized = serde_json::to_string(&mappings).unwrap_or_default();
    if let Err(err) = state
        .store
        .set_setting("custom_model_mappings", &serialized)
    {
        return store_error_response(&err);
    }
    set_custom_model_mappings(mappings.clone());
    json_response(StatusCode::OK, json!({ "status": "ok", "data": mappings }))
}

pub async fn analytics(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let days = params
        .get("days")
        .and_then(|d| d.trim().parse::<i64>().ok())
        .filter(|d| *d >= 0)
        .unwrap_or(7);
    match state.store.get_analytics(days) {
        Ok(result) => ok_data(json!(result)),
        Err(err) => store_error_response(&err),
    }
}

pub async fn get_logs(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let limit = params
        .get("limit")
        .and_then(|l| l.parse::<i64>().ok())
        .filter(|l| *l > 0)
        .unwrap_or(100);
    match state.store.list_logs(limit) {
        Ok(logs) => ok_data(json!(logs)),
        Err(err) => store_error_response(&err),
    }
}

pub async fn clear_logs(State(state): State<SharedState>) -> Response {
    match state.store.clear_logs() {
        Ok(()) => json_response(
            StatusCode::OK,
            json!({ "status": "ok", "message": "logs cleared" }),
        ),
        Err(err) => store_error_response(&err),
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct PruneRequest {
    #[serde(default)]
    days: Option<i64>,
    #[serde(default)]
    max_count: Option<i64>,
}

pub async fn prune_logs(
    State(state): State<SharedState>,
    body: Option<Json<PruneRequest>>,
) -> Response {
    let input = body.map(|Json(value)| value).unwrap_or_default();
    let mut deleted = 0i64;
    if input.days.is_some() || input.max_count.is_some() {
        if let Some(days) = input.days.filter(|d| *d > 0) {
            deleted += state.store.prune_logs_older_than(days).unwrap_or(0);
        }
        if let Some(max_count) = input.max_count.filter(|m| *m > 0) {
            deleted += state.store.prune_logs_limit(max_count).unwrap_or(0);
        }
    } else {
        match state.store.auto_prune_logs() {
            Ok(count) => deleted = count,
            Err(err) => return store_error_response(&err),
        }
    }
    let logs_count = state.store.count_logs().unwrap_or(0);
    json_response(
        StatusCode::OK,
        json!({ "status": "ok", "deleted": deleted, "logs_count": logs_count }),
    )
}

pub async fn system_info(State(state): State<SharedState>) -> Response {
    let retention_raw = state
        .store
        .get_setting("log_retention_days", "7")
        .unwrap_or_else(|_| "7".into());
    let mut retention_days = retention_raw.parse::<i64>().unwrap_or(7);
    if retention_days <= 0 && retention_raw != "0" {
        retention_days = 7;
    }
    let max_raw = state
        .store
        .get_setting("log_max_count", "5000")
        .unwrap_or_else(|_| "5000".into());
    let mut max_count = max_raw.parse::<i64>().unwrap_or(5000);
    if max_count <= 0 && max_raw != "0" {
        max_count = 5000;
    }
    let logs_count = state.store.count_logs().unwrap_or(0);
    let db_path = crate::datadir::database_path();
    let db_size = std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);

    let hide_dock_icon = state
        .store
        .get_setting("hide_dock_icon", "false")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);

    ok_data(json!({
        "data_dir": crate::datadir::path().to_string_lossy(),
        "db_path": db_path.to_string_lossy(),
        "db_size_bytes": db_size,
        "log_retention_days": retention_days,
        "log_max_count": max_count,
        "logs_count": logs_count,
        "routing_strategy": route_strategy(&state.store, &HashMap::new()),
        "hide_dock_icon": hide_dock_icon,
        "dock_supported": cfg!(target_os = "macos"),
    }))
}

#[derive(Debug, Deserialize, Default)]
pub struct SettingsRequest {
    #[serde(default)]
    log_retention_days: Option<i64>,
    #[serde(default)]
    log_max_count: Option<i64>,
    #[serde(default)]
    routing_strategy: Option<String>,
    #[serde(default)]
    hide_dock_icon: Option<bool>,
}

pub async fn update_settings(
    State(state): State<SharedState>,
    Json(input): Json<SettingsRequest>,
) -> Response {
    let strategy = input.routing_strategy.map(|value| value.trim().to_string());
    if let Some(strategy) = strategy.as_deref() {
        if strategy != ROUTE_SMART_QUALITY
            && strategy != ROUTE_ROUND_ROBIN
            && strategy != ROUTE_LATENCY
        {
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "error": "invalid routing strategy" }),
            );
        }
    }
    if input.hide_dock_icon.is_some() && !cfg!(target_os = "macos") {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "隐藏 Dock 图标仅支持 macOS" }),
        );
    }

    let mut updates = Vec::new();
    if let Some(days) = input.log_retention_days {
        updates.push(("log_retention_days".to_string(), days.to_string()));
    }
    if let Some(max_count) = input.log_max_count {
        updates.push(("log_max_count".to_string(), max_count.to_string()));
    }
    if let Some(hide_dock) = input.hide_dock_icon {
        updates.push(("hide_dock_icon".to_string(), hide_dock.to_string()));
    }
    if let Some(strategy) = strategy {
        updates.push(("routing_strategy".to_string(), strategy));
    }
    let changed = input.log_retention_days.is_some() || input.log_max_count.is_some();
    if let Err(err) = state.store.set_settings(&updates) {
        return store_error_response(&err);
    }
    if changed {
        let _ = state.store.auto_prune_logs();
    }
    if let Some(hide_dock) = input.hide_dock_icon {
        crate::settings_events::notify_setting_change("hide_dock_icon", &hide_dock.to_string());
    }
    json_response(StatusCode::OK, json!({ "status": "ok" }))
}

/// Resolves the routing strategy from the query string or stored settings.
pub fn route_strategy(store: &Store, params: &HashMap<String, String>) -> String {
    if let Some(strategy) = params.get("strategy") {
        let strategy = strategy.trim();
        if !strategy.is_empty() {
            return strategy.to_string();
        }
    }
    match store.get_setting("routing_strategy", ROUTE_LATENCY) {
        Ok(value)
            if value == ROUTE_LATENCY
                || value == ROUTE_SMART_QUALITY
                || value == ROUTE_ROUND_ROBIN =>
        {
            value
        }
        _ => ROUTE_LATENCY.to_string(),
    }
}

pub async fn candidates(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let model = params
        .get("model")
        .map(|m| m.trim().to_string())
        .unwrap_or_default();
    let strategy = route_strategy(&state.store, &params);
    match state.router.candidates(&model, &strategy) {
        Ok(channels) => ok_data(json!(channels
            .iter()
            .map(sanitize_channel)
            .collect::<Vec<_>>())),
        Err(err) => json_response(StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": err })),
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct UpstreamModelsRequest {
    #[serde(default)]
    channel_id: Option<i64>,
    #[serde(default)]
    endpoint: String,
    #[serde(default)]
    api_key: String,
    #[serde(default)]
    provider: String,
}

/// Probes candidate `/models` URLs and returns the discovered model ids.
pub async fn fetch_upstream_models(
    State(state): State<SharedState>,
    Json(mut req): Json<UpstreamModelsRequest>,
) -> Response {
    let lease = if let Some(id) = req.channel_id {
        let channel = match state.store.get(id) {
            Ok(c) if c.enabled => c,
            _ => return json_response(StatusCode::CONFLICT, json!({"error":"渠道不可用"})),
        };
        if req.endpoint.trim().is_empty() {
            req.endpoint = channel.endpoint.clone();
        }
        if req.provider.trim().is_empty() {
            req.provider = channel.provider.clone();
        }
        if req.api_key.trim().is_empty() {
            if !crate::probe::same_endpoint(&req.endpoint, &channel.endpoint) {
                return json_response(
                    StatusCode::BAD_REQUEST,
                    json!({"error":"Base URL 已修改，请提供新地址的 API Key 后获取模型"}),
                );
            }
            let lease = match state.gateway.key_pool.acquire_for_channel(
                &channel,
                "",
                &std::collections::HashSet::new(),
                None,
            ) {
                Ok(Some(lease)) => lease,
                _ => {
                    return json_response(
                        StatusCode::CONFLICT,
                        json!({"error":"没有可用于获取模型的 Key"}),
                    )
                }
            };
            req.api_key = lease.key.secret.clone();
            Some(lease)
        } else {
            None
        }
    } else {
        None
    };
    let mut endpoint = req.endpoint.trim().to_string();
    if endpoint.is_empty() {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "Base URL 不能为空" }),
        );
    }
    if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
        endpoint = format!("https://{endpoint}");
    }
    endpoint = endpoint.trim_end_matches('/').to_string();

    let mut candidate_urls: Vec<String> = Vec::new();
    if endpoint.ends_with("/models") {
        candidate_urls.push(endpoint.clone());
    } else {
        candidate_urls.push(format!("{endpoint}/models"));
        if !endpoint.ends_with("/v1") {
            candidate_urls.push(format!("{endpoint}/v1/models"));
        }
    }

    let client = match reqwest::Client::builder()
        .redirect(crate::probe::upstream_redirect_policy())
        .timeout(Duration::from_secs(12))
        .build()
    {
        Ok(client) => client,
        Err(_err) => {
            return json_response(
                StatusCode::BAD_GATEWAY,
                json!({ "error": "无法创建上游请求客户端" }),
            )
        }
    };

    let mut last_err: Option<String> = None;
    let mut models: Vec<String> = Vec::new();

    for target in &candidate_urls {
        let mut request = client
            .get(target)
            .header("accept", "application/json")
            .header("user-agent", "HeiGate/1.0");
        let api_key = req.api_key.trim();
        if !api_key.is_empty() && !api_key.contains("***") {
            request = request
                .header("authorization", format!("Bearer {api_key}"))
                .header("x-api-key", api_key);
        }
        if req.provider == "anthropic" {
            request = request.header("anthropic-version", "2023-06-01");
        }

        let response = match request.send().await {
            Ok(response) => response,
            Err(_err) => {
                last_err = Some("无法连接上游服务".to_string());
                continue;
            }
        };
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .map(crate::api::forward::parse_retry_after)
            .unwrap_or_default();
        let body = match read_upstream_body(response).await {
            Ok(body) => body,
            Err(()) => {
                last_err = Some("上游响应过大或读取失败".to_string());
                break;
            }
        };

        if status != 200 {
            if let Some(lease) = &lease {
                if status == 429 || status == 401 {
                    let mut info = crate::resilience::classify_status(status as i32);
                    info.retry_after = retry_after;
                    let _ = lease.failure(&info, "");
                    return json_response(
                        StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
                        json!({"error":format!("模型列表请求失败：HTTP {status}，Key 已暂避")}),
                    );
                }
            }
            let value: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            let message = value
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .or_else(|| value.get("message").and_then(Value::as_str))
                .unwrap_or("");
            last_err = Some(if message.is_empty() {
                format!("上游返回 HTTP {status}")
            } else {
                format!("上游提示: {message} (HTTP {status})")
            });
            if status == 404 && candidate_urls.len() > 1 {
                continue;
            }
            break;
        }

        models = parse_model_list(&body);
        if models.is_empty() {
            last_err = Some("上游响应成功，但未解析出任何模型 ID".to_string());
        } else {
            last_err = None;
            break;
        }
    }

    match last_err {
        Some(err) => json_response(StatusCode::BAD_GATEWAY, json!({ "error": err })),
        None => ok_data(json!(models)),
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct DetectProtocolRequest {
    #[serde(default)]
    pub channel_id: Option<i64>,
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub model: Option<String>,
}

/// Automatically tests the endpoint for OpenAI and Anthropic protocol support.
pub async fn detect_protocol(
    State(state): State<SharedState>,
    Json(mut req): Json<DetectProtocolRequest>,
) -> Response {
    let _lease = if let Some(id) = req.channel_id {
        let channel = match state.store.get(id) {
            Ok(channel) if channel.enabled => channel,
            _ => return json_response(StatusCode::CONFLICT, json!({"error":"渠道不可用"})),
        };
        if req.endpoint.trim().is_empty() {
            req.endpoint = channel.endpoint.clone();
        }
        if req.model.as_deref().unwrap_or("").trim().is_empty() {
            req.model = Some(channel.primary_model.clone());
        }
        if req.api_key.trim().is_empty() {
            if !crate::probe::same_endpoint(&req.endpoint, &channel.endpoint) {
                return json_response(
                    StatusCode::BAD_REQUEST,
                    json!({"error":"Base URL 已修改，请提供新地址的 API Key 后检测"}),
                );
            }
            let lease = match state.gateway.key_pool.acquire_for_channel(
                &channel,
                "",
                &std::collections::HashSet::new(),
                None,
            ) {
                Ok(Some(lease)) => lease,
                _ => {
                    return json_response(
                        StatusCode::CONFLICT,
                        json!({"error":"没有可用于协议检测的 Key"}),
                    )
                }
            };
            req.api_key = lease.key.secret.clone();
            Some(lease)
        } else {
            None
        }
    } else {
        None
    };

    let mut endpoint = req.endpoint.trim().to_string();
    if endpoint.is_empty() {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "Base URL 不能为空" }),
        );
    }
    if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
        endpoint = format!("https://{endpoint}");
    }
    endpoint = endpoint.trim_end_matches('/').to_string();

    let client = match reqwest::Client::builder()
        .redirect(crate::probe::upstream_redirect_policy())
        .timeout(Duration::from_secs(15))
        .build()
    {
        Ok(c) => c,
        Err(_) => {
            return json_response(
                StatusCode::BAD_GATEWAY,
                json!({ "error": "无法创建探测客户端" }),
            )
        }
    };

    let api_key = req.api_key.trim();
    let mut models: Vec<String> = Vec::new();

    // 1. Probe OpenAI endpoint (/v1/models and /models)
    let candidate_openai_urls = if endpoint.ends_with("/models") {
        vec![endpoint.clone()]
    } else if endpoint.ends_with("/v1") {
        vec![format!("{endpoint}/models")]
    } else {
        let mut u = vec![format!("{endpoint}/v1/models")];
        if !endpoint.ends_with("/v1") {
            u.push(format!("{endpoint}/models"));
        }
        u
    };

    for target in &candidate_openai_urls {
        let mut request = client
            .get(target)
            .header("accept", "application/json")
            .header("user-agent", "HeiGate/1.0");
        if !api_key.is_empty() && !api_key.contains("***") {
            request = request.header("authorization", format!("Bearer {api_key}"));
        }
        if let Ok(res) = request.send().await {
            let status = res.status();
            if let Ok(body) = read_upstream_body(res).await {
                if status.is_success() {
                    let parsed = parse_model_list(&body);
                    if !parsed.is_empty() {
                        models = parsed;
                        break;
                    }
                }
            }
        }
    }

    let model = req
        .model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| models.first().map(String::as_str));
    let base = endpoint.strip_suffix("/models").unwrap_or(&endpoint);
    let base = if base.ends_with("/v1") {
        base.to_string()
    } else {
        format!("{base}/v1")
    };
    // The models route alone does not establish Chat support (Responses-only
    // servers expose it too). Probe each generation route independently.
    let (openai, anthropic, responses) = tokio::join!(
        probe_protocol(
            &client,
            &base,
            api_key,
            "chat/completions",
            json!({
                "model": model.unwrap_or("gpt-4o-mini"), "messages": [{"role":"user","content":"ping"}], "max_tokens":16
            })
        ),
        probe_protocol(
            &client,
            &base,
            api_key,
            "messages",
            json!({
                "model": model.unwrap_or("claude-3-7-sonnet-latest"), "messages": [{"role":"user","content":"ping"}], "max_tokens":16
            })
        ),
        probe_protocol(
            &client,
            &base,
            api_key,
            "responses",
            json!({
                "model": model.unwrap_or("gpt-4o-mini"), "input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"ping"}]}], "max_output_tokens":16
            })
        )
    );
    let detected_protocol = if openai.0 == Some(true) {
        Some("openai-compatible")
    } else if anthropic.0 == Some(true) {
        Some("anthropic")
    } else if responses.0 == Some(true) {
        Some("openai-responses")
    } else {
        None
    };
    let summary = format!(
        "Chat：{}；Anthropic：{}；Responses：{}",
        openai.1, anthropic.1, responses.1
    );

    ok_data(json!({
        "detected_protocol": detected_protocol,
        "supports_openai": openai.0,
        "supports_anthropic": anthropic.0,
        "supports_responses": responses.0,
        "models": models,
        "message": summary
    }))
}

async fn probe_protocol(
    client: &reqwest::Client,
    base: &str,
    key: &str,
    path: &str,
    body: Value,
) -> (Option<bool>, String) {
    let mut request = client
        .post(format!("{base}/{path}"))
        .header("accept", "application/json")
        .header("user-agent", "HeiGate/1.0")
        .json(&body);
    if !key.is_empty() && !key.contains("***") {
        request = request.bearer_auth(key);
        if path == "messages" {
            request = request.header("x-api-key", key);
        }
    }
    if path == "messages" {
        request = request.header("anthropic-version", "2023-06-01");
    }
    let response = match request.send().await {
        Ok(response) => response,
        Err(error) => {
            return (
                None,
                if error.is_timeout() {
                    "超时，未确认"
                } else {
                    "连接失败，未确认"
                }
                .into(),
            )
        }
    };
    let status = response.status().as_u16();
    let body = match read_upstream_body(response).await {
        Ok(body) => body,
        Err(_) => return (None, "响应读取失败，未确认".into()),
    };
    let support = classify_protocol_probe(status, &body, path);
    let message = match support {
        Some(true) if (200..300).contains(&status) => format!("支持 (HTTP {status})"),
        Some(true) => format!("接口存在，但请求未成功 (HTTP {status})"),
        Some(false) => format!("接口不存在或不接受 POST (HTTP {status})"),
        None => format!("未确认 (HTTP {status})"),
    };
    (support, message)
}

fn classify_protocol_probe(status: u16, body: &[u8], path: &str) -> Option<bool> {
    let value = serde_json::from_slice::<Value>(body).ok();
    if (200..300).contains(&status) {
        let valid = value.as_ref().is_some_and(|v| match path {
            "chat/completions" => v.get("choices").is_some_and(Value::is_array),
            "messages" => {
                v.get("type").and_then(Value::as_str) == Some("message")
                    && v.get("content").is_some_and(Value::is_array)
            }
            "responses" => v.get("output").is_some_and(|output| {
                output.is_array()
                    || (v.get("object").and_then(Value::as_str) == Some("response")
                        && output.get("type").and_then(Value::as_str) == Some("message")
                        && output.get("content").is_some())
            }),
            _ => false,
        });
        return valid.then_some(true);
    }
    // Auth/rate-limit/proxy failures can happen before routing, so are not
    // proof of either support or lack of support. Never expose raw error text.
    if let Some(value) = value.as_ref() {
        let error = value.get("error").unwrap_or(value);
        let text = [
            error.get("code"),
            error.get("type"),
            error.get("message"),
            value.get("message"),
        ]
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
        if matches!(status, 400 | 404 | 422) && (text.contains("model") || text.contains("模型"))
        {
            return Some(true);
        }
        if status == 404
            && (text.contains("route_not_found")
                || text.contains("endpoint not found")
                || text.contains("cannot post"))
        {
            return Some(false);
        }
    }
    if status == 405
        || (status == 404
            && (body.is_empty()
                || String::from_utf8_lossy(body)
                    .trim()
                    .eq_ignore_ascii_case("not found")))
    {
        return Some(false);
    }
    None
}

fn parse_model_list(body: &[u8]) -> Vec<String> {
    let mut models: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return models;
    };

    let items = value
        .get("data")
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
        .or_else(|| value.get("models").and_then(Value::as_array));
    if let Some(items) = items {
        for item in items {
            let name = match item {
                Value::String(text) => text.trim().to_string(),
                Value::Object(map) => map
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .or_else(|| map.get("name").and_then(Value::as_str))
                    .unwrap_or("")
                    .trim()
                    .to_string(),
                _ => String::new(),
            };
            if !name.is_empty() && seen.insert(name.clone()) {
                models.push(name);
            }
        }
        return models;
    }

    if let Ok(direct) = serde_json::from_slice::<Vec<String>>(body) {
        for model in direct {
            let model = model.trim().to_string();
            if !model.is_empty() {
                models.push(model);
            }
        }
    }
    models
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ClientConfigPayload {
    #[serde(default)]
    pub strategy: Option<String>,
    #[serde(default)]
    pub allowed_channels: Option<Vec<String>>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub small_model: Option<String>,
    #[serde(default)]
    pub allowed_models: Option<Vec<String>>,
    #[serde(default)]
    pub auto_enabled: Option<bool>,
    #[serde(default)]
    pub auto_strategy: Option<String>,
    #[serde(default)]
    pub auto_models: Option<Vec<String>>,
    #[serde(default)]
    pub apply_local: Option<bool>,
    #[serde(default)]
    pub restart: Option<bool>,
    #[serde(default)]
    pub launch_terminal: Option<bool>,
}

impl ClientConfigPayload {
    fn effective_models(&self) -> Option<&[String]> {
        if self.auto_enabled.unwrap_or(false) {
            if let Some(models) = self.auto_models.as_deref().filter(|m| !m.is_empty()) {
                return Some(models);
            }
        }
        self.allowed_models.as_deref()
    }
}

pub fn apply_hermes_local_config(
    gateway_key: &str,
    gateway_port: u16,
    model: &str,
    allowed_models: &[String],
) -> Result<(), std::io::Error> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let hermes_dir = std::path::PathBuf::from(&home).join(".hermes");
    apply_hermes_config_in_dir(
        &hermes_dir,
        gateway_key,
        &format!("http://127.0.0.1:{gateway_port}/hermes/v1"),
        model,
        allowed_models,
    )
}

/// Writes `content` to a sibling temp file and renames it over `path`, so a
/// crash or full disk can never leave a half-written config behind. Existing
/// file permissions are preserved.
fn write_atomic(path: &std::path::Path, content: &str) -> std::io::Result<()> {
    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(".heigate.tmp");
    let tmp = path.with_file_name(tmp_name);
    std::fs::write(&tmp, content)?;
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Atomically writes secret-bearing files with owner-only permissions on Unix.
/// The temp file is secured before any secret bytes are written.
fn write_atomic_private(path: &std::path::Path, content: &str) -> std::io::Result<()> {
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(".heigate.tmp");
    let tmp = path.with_file_name(tmp_name);
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        #[cfg(unix)]
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn restore_atomic_file(path: &std::path::Path, original: Option<&str>) -> std::io::Result<()> {
    match original {
        Some(content) => write_atomic(path, content),
        None => match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        },
    }
}

fn yaml_section<'a>(
    map: &'a mut serde_yaml_ng::Mapping,
    key: &str,
) -> std::io::Result<&'a mut serde_yaml_ng::Mapping> {
    use serde_yaml_ng::Value as Yaml;
    let value = map.entry(Yaml::String(key.into())).or_insert(Yaml::Null);
    if value.is_null() {
        *value = Yaml::Mapping(Default::default());
    }
    value.as_mapping_mut().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Hermes 配置中的 {key} 必须是 YAML 映射"),
        )
    })
}

fn merge_hermes_config(
    content: &str,
    base_url: &str,
    model: &str,
    allowed_models: &[String],
) -> std::io::Result<String> {
    use serde_yaml_ng::{Mapping, Value as Yaml};
    let invalid = |err| std::io::Error::new(std::io::ErrorKind::InvalidData, err);
    let mut document: Yaml = serde_yaml_ng::from_str(content).map_err(invalid)?;
    if document.is_null() {
        document = Yaml::Mapping(Mapping::new());
    }
    let root = document.as_mapping_mut().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Hermes 配置根节点必须是 YAML 映射",
        )
    })?;
    let key = |s: &str| Yaml::String(s.to_string());
    // Some installations start with a scalar model name, others a model mapping.
    if root.get(key("model")).is_some_and(Yaml::is_string) {
        root.insert(key("model"), Yaml::Mapping(Mapping::new()));
    }
    let config = yaml_section(root, "model")?;
    for (name, value) in [
        ("default", model),
        ("provider", "bend"),
        ("base_url", base_url),
        ("key_env", "HERMES_CUSTOM_BEND_API_KEY"),
        ("api_mode", "chat_completions"),
    ] {
        config.insert(key(name), key(value));
    }
    config.remove(key("api_key"));
    let bend = yaml_section(yaml_section(root, "providers")?, "bend")?;
    for (name, value) in [
        ("name", "HeiGate"),
        ("model", model),
        ("base_url", base_url),
        ("key_env", "HERMES_CUSTOM_BEND_API_KEY"),
        ("api_mode", "chat_completions"),
    ] {
        bend.insert(key(name), key(value));
    }
    bend.remove(key("api_key"));
    bend.insert(key("discover_models"), Yaml::Bool(true));
    let old_models = bend
        .get(key("models"))
        .and_then(Yaml::as_mapping)
        .cloned()
        .unwrap_or_default();
    let mut models = Mapping::new();
    // The configured default must also be advertised when it is the Auto alias.
    for name in std::iter::once(model).chain(allowed_models.iter().map(String::as_str)) {
        models.insert(
            key(name),
            old_models
                .get(key(name))
                .cloned()
                .unwrap_or(Yaml::Mapping(Mapping::new())),
        );
    }
    bend.insert(key("models"), Yaml::Mapping(models));
    root.entry(key("fallback_providers"))
        .or_insert(Yaml::Sequence(Vec::new()));
    serde_yaml_ng::to_string(&document).map_err(invalid)
}

fn apply_hermes_config_in_dir(
    hermes_dir: &std::path::Path,
    gateway_key: &str,
    base_url: &str,
    model: &str,
    allowed_models: &[String],
) -> Result<(), std::io::Error> {
    std::fs::create_dir_all(hermes_dir)?;
    let config_path = hermes_dir.join("config.yaml");
    let env_path = hermes_dir.join(".env");
    let original = if config_path.exists() {
        Some(std::fs::read_to_string(&config_path)?)
    } else {
        None
    };
    let merged = merge_hermes_config(
        original.as_deref().unwrap_or(""),
        base_url,
        model,
        allowed_models,
    )?;
    let existed = env_path.exists();
    let env_content = if existed {
        std::fs::read_to_string(&env_path)?
    } else {
        String::new()
    };
    let key_line = format!("HERMES_CUSTOM_BEND_API_KEY={gateway_key}");
    let re_key = regex::Regex::new(r"(?m)^HERMES_CUSTOM_BEND_API_KEY=.*$").unwrap();
    let new_env = if re_key.is_match(&env_content) {
        re_key
            .replace_all(&env_content, regex::NoExpand(&key_line))
            .to_string()
    } else if env_content.is_empty() {
        format!("{key_line}\n")
    } else {
        format!("{}\n{key_line}\n", env_content.trim_end_matches('\n'))
    };
    // Parse and prepare both files before changing either; invalid YAML or an
    // unreadable .env must leave the user's existing configuration untouched.
    if let Some(original) = original.as_deref() {
        let backup = hermes_dir.join("config.yaml.heigate.bak");
        if !backup.exists() {
            std::fs::write(backup, original)?;
        }
    }
    let write_result = write_atomic(&config_path, &merged)
        .and_then(|()| write_atomic_private(&env_path, &new_env));
    if let Err(error) = write_result {
        let rollback = restore_atomic_file(&config_path, original.as_deref());
        if rollback.is_err() {
            return Err(std::io::Error::new(
                error.kind(),
                format!("Hermes 配置写入失败: {error}；配置回滚失败，请检查 config.yaml"),
            ));
        }
        return Err(error);
    }
    Ok(())
}

/// Platforms that may have a dedicated `/{client}/v1/*` entry point and a
/// stored per-client config. Anything else is rejected instead of silently
/// falling back to default settings.
pub const KNOWN_CLIENTS: [&str; 5] = [
    "hermes",
    "codex",
    "claude_desktop",
    "claude_code",
    "generic",
];

/// Normalises a client id the same way settings keys are derived
/// (case-insensitive, `-` and `_` interchangeable).
pub fn normalize_client_id(client_id: &str) -> String {
    client_id.to_lowercase().replace('-', "_")
}

fn is_known_client(client_id: &str) -> bool {
    KNOWN_CLIENTS.contains(&normalize_client_id(client_id).as_str())
}

fn unknown_client_response(client_id: &str) -> Response {
    json_response(
        StatusCode::NOT_FOUND,
        json!({ "error": format!("未知的客户端: {client_id}") }),
    )
}

pub fn get_client_config(store: &Store, client_id: &str) -> ClientConfigPayload {
    let key = format!(
        "client_config_{}",
        client_id.to_lowercase().replace('-', "_")
    );
    if let Ok(raw) = store.get_setting(&key, "{}") {
        if let Ok(cfg) = serde_json::from_str::<ClientConfigPayload>(&raw) {
            return cfg;
        }
    }
    ClientConfigPayload::default()
}

pub async fn get_client_configs(State(state): State<SharedState>) -> Response {
    let platforms = KNOWN_CLIENTS;
    let mut map = serde_json::Map::new();
    for p in platforms {
        let cfg = get_client_config(&state.store, p);
        map.insert(
            p.to_string(),
            serde_json::to_value(cfg).unwrap_or(Value::Null),
        );
    }
    ok_data(Value::Object(map))
}

#[derive(Debug, Deserialize)]
pub struct CodexApplyRequest {
    pub model: String,
    #[serde(default = "default_codex_mode")]
    pub mode: String,
    #[serde(default)]
    pub channel_id: Option<i64>,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub restart: bool,
}

fn default_codex_mode() -> String {
    "gateway".into()
}

pub async fn get_codex_config_status() -> Response {
    match tokio::task::spawn_blocking(crate::api::codex::status).await {
        Ok(status) => ok_data(serde_json::to_value(status).unwrap_or(Value::Null)),
        Err(error) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("读取 Codex 配置状态失败: {error}") }),
        ),
    }
}

pub async fn apply_codex_config(
    State(state): State<SharedState>,
    Json(request): Json<CodexApplyRequest>,
) -> Response {
    let model = request.model.trim().to_string();
    if model.is_empty() || model.len() > 256 {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "Codex 模型不能为空且最多 256 个字符" }),
        );
    }
    let mode = request.mode.trim().to_ascii_lowercase();
    if mode != "gateway" && mode != "direct" {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "Codex 接入模式无效" }),
        );
    }
    if mode == "direct" && request.models.is_empty() {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "直连模式至少需要一个模型" }),
        );
    }
    let direct_credentials = if mode == "direct" {
        if let Some(channel_id) = request.channel_id {
            let channel = match state.store.get(channel_id) {
                Ok(channel) => channel,
                Err(err) => return store_error_response(&err),
            };
            if !channel.enabled {
                return json_response(
                    StatusCode::BAD_REQUEST,
                    json!({ "error": "所选渠道已禁用，无法配置 Codex 直连" }),
                );
            }
            if channel.provider != "openai" {
                return json_response(
                    StatusCode::BAD_REQUEST,
                    json!({ "error": "Codex 直连只支持 OpenAI 兼容或 Responses 渠道" }),
                );
            }
            if !channel.has_model(&model)
                || request.models.iter().any(|model| !channel.has_model(model))
            {
                return json_response(
                    StatusCode::BAD_REQUEST,
                    json!({ "error": "Codex 模型目录只能包含所选渠道已配置的模型" }),
                );
            }
            let now = chrono::Utc::now();
            let cooling_keys = match state.store.model_cooling_key_ids(channel_id, &model) {
                Ok(keys) => keys,
                Err(err) => return store_error_response(&err),
            };
            let key = match state.store.list_keys(channel_id) {
                Ok(keys) => keys.into_iter().find(|key| {
                    key.enabled
                        && !key.auth_invalid
                        && !cooling_keys.contains(&key.id)
                        && !key.cooldown_until.is_some_and(|until| until > now)
                        && !key.secret.trim().is_empty()
                }),
                Err(err) => return store_error_response(&err),
            };
            let Some(key) = key else {
                return json_response(
                    StatusCode::BAD_REQUEST,
                    json!({ "error": "所选渠道没有可用 Key，请启用一把未冷却的 Key 后重试" }),
                );
            };
            (Some(channel.endpoint), Some(key.secret))
        } else {
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "error": "直连模式必须选择 HeiGate 中已保存的渠道，密钥不会通过前端明文提交" }),
            );
        }
    } else {
        (None, None)
    };
    let options = crate::api::codex::CodexApplyOptions {
        model,
        gateway_base_url: format!("http://127.0.0.1:{}/codex/v1", state.gateway_port),
        gateway_key: state.gateway_key.clone(),
        mode,
        direct_base_url: direct_credentials.0,
        direct_key: direct_credentials.1,
        direct_models: request.models,
    };
    let restart = request.restart;
    match tokio::task::spawn_blocking(move || {
        let status = crate::api::codex::apply(&options)?;
        let restart_result = if restart {
            Some(crate::api::codex::restart())
        } else {
            None
        };
        Ok::<_, String>((status, restart_result))
    })
    .await
    {
        Ok(Ok((status, restart_message))) => ok_data(json!({
            "status": "ok",
            "message": codex_apply_message(options_mode_label(&status), restart_message),
            "config": status,
        })),
        Ok(Err(error)) => json_response(StatusCode::BAD_REQUEST, json!({ "error": error })),
        Err(error) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("应用 Codex 配置失败: {error}") }),
        ),
    }
}

fn options_mode_label(status: &crate::api::codex::CodexConfigStatus) -> &'static str {
    if status.provider.as_deref() == Some("heigate_direct") {
        "direct"
    } else {
        "gateway"
    }
}

fn codex_apply_message(mode: &str, restart: Option<Result<String, String>>) -> String {
    let mode_label = if mode == "direct" { "直连" } else { "网关" };
    match restart {
        Some(Ok(message)) => format!("已应用 Codex {mode_label}配置。{message}"),
        Some(Err(error)) => format!(
            "已应用 Codex {mode_label}配置，但自动重启失败：{error}。请手动重新打开 Codex。"
        ),
        None => "已应用 Codex 配置。请重启 Codex 使配置和模型目录生效。".to_string(),
    }
}

pub async fn restore_codex_config() -> Response {
    match tokio::task::spawn_blocking(|| {
        let status = crate::api::codex::restore()?;
        let restart = crate::api::codex::restart();
        Ok::<_, String>((status, restart))
    })
    .await
    {
        Ok(Ok((status, restart))) => {
            let message = match restart {
                Ok(message) => format!("已恢复接管前的 Codex 配置。{message}"),
                Err(error) => format!(
                    "已恢复接管前的 Codex 配置，但自动重启失败：{error}。请手动重新打开 Codex。"
                ),
            };
            ok_data(json!({
                "status": "ok",
                "message": message,
                "config": status,
            }))
        }
        Ok(Err(error)) => json_response(StatusCode::BAD_REQUEST, json!({ "error": error })),
        Err(error) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("恢复 Codex 配置失败: {error}") }),
        ),
    }
}

/// Applies the local side effects of saving the Hermes client config (config
/// rewrite, gateway restart, terminal launch). Blocking by nature — file I/O
/// and child processes — so callers must run it via `spawn_blocking`.
fn run_hermes_actions(
    payload: &ClientConfigPayload,
    gateway_key: &str,
    gateway_port: u16,
) -> Result<String, String> {
    run_hermes_actions_with(
        payload,
        || apply_hermes_payload(payload, gateway_key, gateway_port),
        |terminal| {
            if terminal {
                launch_hermes_terminal()
            } else {
                let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
                let hermes_bin = std::path::PathBuf::from(home).join(".local/bin/hermes");
                let bin = if hermes_bin.exists() {
                    hermes_bin
                } else {
                    "hermes".into()
                };
                checked_command(
                    std::process::Command::new(bin)
                        .arg("gateway")
                        .arg("restart"),
                )
            }
        },
    )
}

fn checked_command(command: &mut std::process::Command) -> Result<(), String> {
    let out = command.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

fn launch_hermes_terminal() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        checked_command(
            std::process::Command::new("osascript")
                .arg("-e")
                .arg("tell application \"Terminal\" to do script \"hermes\""),
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("当前平台不支持自动打开终端，请手动运行 hermes".into())
    }
}

fn run_hermes_actions_with(
    payload: &ClientConfigPayload,
    apply: impl FnOnce() -> Result<(), String>,
    mut launch: impl FnMut(bool) -> Result<(), String>,
) -> Result<String, String> {
    let mut msg = String::new();
    if payload.apply_local.unwrap_or(false) {
        apply().map_err(|e| format!("更新 Hermes 本地配置失败: {e}"))?;
        msg.push_str("已更新 Hermes 本地配置。");
    }
    if payload.restart.unwrap_or(false) {
        launch(false).map_err(|e| format!("Hermes 重启失败: {e}"))?;
        msg.push_str(" 已成功重启 Hermes 网关服务。");
    }
    if payload.launch_terminal.unwrap_or(false) {
        launch(true).map_err(|e| format!("启动 Hermes 终端失败: {e}"))?;
        msg.push_str(" 已在终端启动 Hermes 对话。");
    }
    Ok(msg.trim().to_string())
}

fn apply_hermes_payload(
    payload: &ClientConfigPayload,
    gateway_key: &str,
    gateway_port: u16,
) -> Result<(), String> {
    if payload.apply_local.unwrap_or(false) {
        let auto_on = payload.auto_enabled.unwrap_or(false);
        let m = if auto_on {
            "Auto"
        } else {
            payload.model.as_deref().unwrap_or("Auto")
        };
        let am = if auto_on
            && payload
                .auto_models
                .as_ref()
                .map(|m| !m.is_empty())
                .unwrap_or(false)
        {
            payload.auto_models.as_deref().unwrap_or(&[])
        } else {
            payload.allowed_models.as_deref().unwrap_or(&[])
        };
        apply_hermes_local_config(gateway_key, gateway_port, m, am).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub async fn update_client_config(
    State(state): State<SharedState>,
    Path(client_id): Path<String>,
    Json(payload): Json<ClientConfigPayload>,
) -> Response {
    if !is_known_client(&client_id) {
        return unknown_client_response(&client_id);
    }
    let client_id = normalize_client_id(&client_id);
    if client_id != "hermes"
        && (payload.apply_local.unwrap_or(false)
            || payload.restart.unwrap_or(false)
            || payload.launch_terminal.unwrap_or(false))
    {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({"error": "此客户端仅支持保存网关设置，请手动配置和启动客户端"}),
        );
    }
    let key = format!("client_config_{client_id}");
    let serialized = match serde_json::to_string(&payload) {
        Ok(s) => s,
        Err(_) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "error": "序列化客户端配置失败" }),
            )
        }
    };
    if let Err(err) = state.store.set_setting(&key, &serialized) {
        return store_error_response(&err);
    }

    if client_id == "hermes" {
        let actions_payload = payload.clone();
        let gateway_key = state.gateway_key.clone();
        let gateway_port = state.gateway_port;
        let msg = tokio::task::spawn_blocking(move || {
            run_hermes_actions(&actions_payload, &gateway_key, gateway_port)
        })
        .await
        .unwrap_or_else(|err| Err(format!("执行 Hermes 操作失败: {err}")));
        return match msg {
            Ok(message) => {
                ok_data(json!({ "status": "ok", "client_id": client_id, "message": message }))
            }
            Err(error) => json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("网关设置已保存，但本地操作未完成：{error}"), "gateway_saved": true }),
            ),
        };
    }

    ok_data(json!({ "status": "ok", "client_id": client_id }))
}

/// Shared gateway forwarding entry point with optional client-specific policy and channel filters.
async fn forward_with_proto_and_client(
    state: &AppState,
    proto: crate::api::forward::GatewayProtocol,
    headers: &HeaderMap,
    path: String,
    body: axum::body::Bytes,
    client: Option<String>,
) -> Response {
    let (strategy, allowed_channels, allowed_models) = if let Some(ref c) = client {
        let cfg = get_client_config(&state.store, c);
        let strat = if cfg.auto_enabled.unwrap_or(false) && cfg.auto_strategy.is_some() {
            match cfg.auto_strategy.as_deref() {
                Some(s)
                    if s == ROUTE_SMART_QUALITY
                        || s == ROUTE_ROUND_ROBIN
                        || s == ROUTE_LATENCY
                        || s == ROUTE_MANUAL_PRIORITY =>
                {
                    s.to_string()
                }
                _ => route_strategy(&state.store, &HashMap::new()),
            }
        } else {
            match cfg.strategy.as_deref() {
                Some(s)
                    if s == ROUTE_SMART_QUALITY
                        || s == ROUTE_ROUND_ROBIN
                        || s == ROUTE_LATENCY
                        || s == ROUTE_MANUAL_PRIORITY =>
                {
                    s.to_string()
                }
                _ => route_strategy(&state.store, &HashMap::new()),
            }
        };
        let models = cfg.effective_models().map(<[String]>::to_vec);
        (strat, cfg.allowed_channels, models)
    } else {
        (route_strategy(&state.store, &HashMap::new()), None, None)
    };

    let ctx = RequestContext {
        client_ip: String::new(),
        user_agent: headers
            .get("user-agent")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string(),
        request_path: path,
        strategy,
        allowed_channels,
        allowed_models,
    };
    state
        .gateway
        .forward(proto, headers, body.to_vec(), &ctx)
        .await
}

async fn forward_with_proto(
    state: &AppState,
    proto: crate::api::forward::GatewayProtocol,
    headers: &HeaderMap,
    path: String,
    body: axum::body::Bytes,
) -> Response {
    forward_with_proto_and_client(state, proto, headers, path, body, Some("generic".into())).await
}

pub async fn forward_messages(
    State(state): State<SharedState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    forward_with_proto(
        &state,
        crate::api::forward::GatewayProtocol::Anthropic,
        &headers,
        "/v1/messages".to_string(),
        body,
    )
    .await
}

pub async fn forward_chat_completions(
    State(state): State<SharedState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    forward_with_proto(
        &state,
        crate::api::forward::GatewayProtocol::Chat,
        &headers,
        "/v1/chat/completions".to_string(),
        body,
    )
    .await
}

pub async fn forward_responses(
    State(state): State<SharedState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    forward_with_proto(
        &state,
        crate::api::forward::GatewayProtocol::Responses,
        &headers,
        "/v1/responses".to_string(),
        body,
    )
    .await
}

pub async fn forward_client_messages(
    State(state): State<SharedState>,
    Path(client): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if !is_known_client(&client) {
        return unknown_client_response(&client);
    }
    forward_with_proto_and_client(
        &state,
        crate::api::forward::GatewayProtocol::Anthropic,
        &headers,
        format!("/{client}/v1/messages"),
        body,
        Some(client),
    )
    .await
}

pub async fn forward_client_chat_completions(
    State(state): State<SharedState>,
    Path(client): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if !is_known_client(&client) {
        return unknown_client_response(&client);
    }
    forward_with_proto_and_client(
        &state,
        crate::api::forward::GatewayProtocol::Chat,
        &headers,
        format!("/{client}/v1/chat/completions"),
        body,
        Some(client),
    )
    .await
}

pub async fn forward_client_responses(
    State(state): State<SharedState>,
    Path(client): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if !is_known_client(&client) {
        return unknown_client_response(&client);
    }
    forward_with_proto_and_client(
        &state,
        crate::api::forward::GatewayProtocol::Responses,
        &headers,
        format!("/{client}/v1/responses"),
        body,
        Some(client),
    )
    .await
}

pub fn build_gateway_models_response(
    channels: &[Channel],
    params: &HashMap<String, String>,
    allowed_models: Option<&[String]>,
    include_auto: bool,
) -> Response {
    let raw = matches!(
        params.get("raw").map(String::as_str),
        Some("true") | Some("1")
    );
    if raw {
        let mut seen = std::collections::HashSet::new();
        let mut data: Vec<Value> = Vec::new();
        let has_enabled = channels.iter().any(|c| c.enabled);
        if has_enabled && include_auto {
            data.push(json!({ "id": AUTO_MODEL_NAME, "object": "model", "owned_by": "heigate" }));
        }
        for ch in channels {
            if !ch.enabled {
                continue;
            }
            let mut models = vec![ch.primary_model.clone()];
            models.extend(ch.extra_models.clone());
            for name in models {
                let name = name.trim().to_string();
                if !name.is_empty() && seen.insert(name.clone()) {
                    data.push(json!({ "id": name, "object": "model", "owned_by": ch.name }));
                }
            }
        }
        if let Some(allowed) = allowed_models {
            if !allowed.is_empty() {
                data.retain(|item| {
                    let id = item["id"].as_str().unwrap_or("");
                    (include_auto && id.eq_ignore_ascii_case(AUTO_MODEL_NAME))
                        || allowed.iter().any(|m| are_models_equivalent(m, id))
                });
            }
        }
        return json_response(StatusCode::OK, json!({ "object": "list", "data": data }));
    }

    // Canonical aggregated models.
    let mut grouped: Vec<(String, Vec<String>)> = Vec::new();
    for ch in channels {
        if !ch.enabled {
            continue;
        }
        let mut models = vec![ch.primary_model.clone()];
        models.extend(ch.extra_models.clone());
        for name in models {
            let name = name.trim().to_string();
            if name.is_empty() {
                continue;
            }
            let key = get_canonical_group_key(&name);
            if key.is_empty() {
                continue;
            }
            match grouped.iter_mut().find(|(k, _)| *k == key) {
                Some((_, aliases)) => aliases.push(name),
                None => grouped.push((key, vec![name])),
            }
        }
    }

    let now = chrono::Utc::now().timestamp();
    let mut data: Vec<Value> = Vec::new();
    if !grouped.is_empty() && include_auto {
        data.push(
            json!({ "id": AUTO_MODEL_NAME, "object": "model", "created": now, "owned_by": "heigate" }),
        );
    }
    let mut seen_canonical = std::collections::HashSet::new();
    for (key, aliases) in &grouped {
        let mut canonical = select_canonical_display_name(aliases);
        if canonical.is_empty() {
            canonical = key.clone();
        }
        if !seen_canonical.insert(canonical.clone()) {
            continue;
        }
        let lower = canonical.to_lowercase();
        let owned_by = if lower.starts_with("deepseek") {
            "deepseek"
        } else if lower.starts_with("claude") {
            "anthropic"
        } else if lower.starts_with("gpt") {
            "openai"
        } else if lower.starts_with("qwen") {
            "qwen"
        } else if lower.starts_with("glm") {
            "zhipu"
        } else if lower.starts_with("kimi") {
            "moonshot"
        } else if lower.starts_with("minimax") {
            "minimax"
        } else if lower.starts_with("grok") {
            "xai"
        } else {
            "heigate"
        };
        data.push(json!({
            "id": canonical,
            "object": "model",
            "created": now,
            "owned_by": owned_by,
        }));
    }
    if let Some(allowed) = allowed_models {
        if !allowed.is_empty() {
            data.retain(|item| {
                let id = item["id"].as_str().unwrap_or("");
                (include_auto && id.eq_ignore_ascii_case(AUTO_MODEL_NAME))
                    || allowed.iter().any(|m| are_models_equivalent(m, id))
            });
        }
    }
    data.sort_by(|a, b| {
        a["id"]
            .as_str()
            .unwrap_or("")
            .cmp(b["id"].as_str().unwrap_or(""))
    });
    json_response(StatusCode::OK, json!({ "object": "list", "data": data }))
}

/// Handles `GET /v1/models`.
pub async fn gateway_models(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    gateway_client_models(State(state), Path("generic".into()), Query(params)).await
}

/// Handles `GET /{client}/v1/models`.
pub async fn gateway_client_models(
    State(state): State<SharedState>,
    Path(client): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    if !is_known_client(&client) {
        return unknown_client_response(&client);
    }
    let cfg = get_client_config(&state.store, &client);
    let channels = match state.store.list() {
        Ok(chans) => chans,
        Err(err) => return store_error_response(&err),
    };
    let filtered_channels: Vec<_> = if let Some(ref allowed) = cfg.allowed_channels {
        if !allowed.is_empty() {
            channels
                .into_iter()
                .filter(|c| allowed.contains(&c.name))
                .collect()
        } else {
            channels
        }
    } else {
        channels
    };
    let auto_on = cfg.auto_enabled.unwrap_or(false);
    build_gateway_models_response(&filtered_channels, &params, cfg.effective_models(), auto_on)
}

struct LogSubscription {
    store: Arc<Store>,
    id: u64,
}

impl Drop for LogSubscription {
    fn drop(&mut self) {
        self.store.unsubscribe(self.id);
    }
}

/// Streams request-log events as SSE.
pub async fn stream_logs(State(state): State<SharedState>) -> Response {
    let (receiver, sub_id) = state.store.subscribe_logs();
    let subscription = LogSubscription {
        store: state.store.clone(),
        id: sub_id,
    };

    let stream = futures_util::stream::unfold(
        (receiver, subscription, false),
        |(mut receiver, subscription, sent_connected)| async move {
            if !sent_connected {
                let frame = "event: connected\ndata: {\"connected\":true}\n\n".to_string();
                return Some((
                    Ok::<_, std::convert::Infallible>(Bytes::from(frame)),
                    (receiver, subscription, true),
                ));
            }
            let mut ticker = tokio::time::interval(Duration::from_secs(15));
            ticker.tick().await; // consume the immediate first tick
            tokio::select! {
                event = receiver.recv() => {
                    match event {
                        Some(event) => {
                            let (kind, payload) = match event.kind.as_str() {
                                "created" => ("log_created", serde_json::to_string(&event.log).unwrap_or_default()),
                                "updated" => ("log_updated", serde_json::to_string(&event.log).unwrap_or_default()),
                                "cleared" => ("logs_cleared", "{}".to_string()),
                                "pruned" => ("logs_pruned", "{}".to_string()),
                                _ => ("message", "{}".to_string()),
                            };
                            let frame = format!("event: {kind}\ndata: {payload}\n\n");
                            Some((Ok(Bytes::from(frame)), (receiver, subscription, true)))
                        }
                        None => None,
                    }
                }
                _ = ticker.tick() => {
                    let frame = format!(
                        "event: ping\ndata: {{\"ts\":{}}}\n\n",
                        chrono::Utc::now().timestamp_millis()
                    );
                    Some((Ok(Bytes::from(frame)), (receiver, subscription, true)))
                }
            }
        },
    );

    let mut response = Response::new(Body::from_stream(stream));
    response.headers_mut().insert(
        "content-type",
        HeaderValue::from_static("text/event-stream"),
    );
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-cache"));
    response
        .headers_mut()
        .insert("connection", HeaderValue::from_static("keep-alive"));
    response
        .headers_mut()
        .insert("x-accel-buffering", HeaderValue::from_static("no"));
    response
}

#[cfg(test)]
mod settings_tests {
    use super::*;
    use crate::api::gateway::Gateway;
    use crate::router::Router as ChannelRouter;
    use crate::runner::ProbeRunner;
    use axum::response::IntoResponse;

    fn test_state() -> SharedState {
        let store = Store::open_in_memory().unwrap();
        let router = Arc::new(ChannelRouter::new(store.clone()));
        Arc::new(AppState {
            runner: Arc::new(ProbeRunner::new(store.clone())),
            gateway: Arc::new(Gateway::new(store.clone(), router.clone())),
            router,
            store,
            gateway_key: "test".into(),
            gateway_port: 28888,
            version: "test".into(),
        })
    }

    #[test]
    fn codex_apply_keeps_configuration_success_separate_from_restart_failure() {
        let message =
            codex_apply_message("gateway", Some(Err("Codex could not be launched".into())));
        assert!(message.contains("已应用 Codex 网关配置"));
        assert!(message.contains("自动重启失败"));
        assert!(message.contains("请手动重新打开 Codex"));
    }

    #[tokio::test]
    async fn invalid_settings_do_not_partially_persist_valid_fields() {
        let state = test_state();
        let response = update_settings(
            State(state.clone()),
            Json(SettingsRequest {
                log_retention_days: Some(30),
                routing_strategy: Some("unknown".into()),
                ..Default::default()
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            state.store.get_setting("log_retention_days", "7").unwrap(),
            "7"
        );
    }

    #[tokio::test]
    async fn valid_settings_are_persisted_together() {
        let state = test_state();
        let response = update_settings(
            State(state.clone()),
            Json(SettingsRequest {
                log_retention_days: Some(30),
                log_max_count: Some(1200),
                routing_strategy: Some(ROUTE_ROUND_ROBIN.into()),
                ..Default::default()
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            state.store.get_setting("log_retention_days", "7").unwrap(),
            "30"
        );
        assert_eq!(
            state.store.get_setting("log_max_count", "5000").unwrap(),
            "1200"
        );
        assert_eq!(
            route_strategy(&state.store, &HashMap::new()),
            ROUTE_ROUND_ROBIN
        );
    }

    #[tokio::test]
    async fn detect_protocol_validates_endpoint() {
        let state = test_state();
        let response = detect_protocol(
            State(state),
            Json(DetectProtocolRequest {
                endpoint: "".into(),
                api_key: "".into(),
                channel_id: None,
                model: None,
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn saved_detection_credentials_are_bound_to_the_saved_endpoint_and_draft_keys_win() {
        let received = Arc::new(std::sync::Mutex::new(Vec::<(String, String)>::new()));
        let seen = received.clone();
        let app = axum::Router::new().fallback(move |headers: HeaderMap| {
            let seen = seen.clone();
            async move {
                seen.lock().unwrap().push((
                    headers.get("authorization").and_then(|h| h.to_str().ok()).unwrap_or("").to_string(),
                    headers.get("x-api-key").and_then(|h| h.to_str().ok()).unwrap_or("").to_string(),
                ));
                Json(json!({"choices":[],"type":"message","content":[],"output":[],"data":[{"id":"model"}]}))
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let endpoint = format!("http://{address}");
        let state = test_state();
        let mut channel = Channel {
            name: "saved".into(),
            enabled: true,
            provider: "openai".into(),
            endpoint: endpoint.clone(),
            primary_model: "model".into(),
            api_key: "saved-secret".into(),
            ..Default::default()
        };
        state.store.save(&mut channel).unwrap();
        let response = detect_protocol(
            State(state.clone()),
            Json(DetectProtocolRequest {
                channel_id: Some(channel.id),
                endpoint: format!("{endpoint}/draft"),
                ..Default::default()
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(received.lock().unwrap().is_empty());
        assert_eq!(state.store.list_keys(channel.id).unwrap()[0].attempts, 0);
        for (url, api_key, expected) in [
            (format!("{endpoint}/v1/"), "", "saved-secret"),
            (format!("{endpoint}/draft"), "draft-secret", "draft-secret"),
            (endpoint, "explicit-secret", "explicit-secret"),
        ] {
            let response = detect_protocol(
                State(state.clone()),
                Json(DetectProtocolRequest {
                    channel_id: Some(channel.id),
                    endpoint: url,
                    api_key: api_key.into(),
                    ..Default::default()
                }),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let requests = std::mem::take(&mut *received.lock().unwrap());
            assert!(!requests.is_empty());
            for (authorization, key) in requests {
                assert_eq!(authorization, format!("Bearer {expected}"));
                if !key.is_empty() {
                    assert_eq!(key, expected);
                }
            }
        }
        assert_eq!(state.store.list_keys(channel.id).unwrap()[0].attempts, 1);
        server.abort();
    }

    #[tokio::test]
    async fn model_listing_uses_draft_endpoints_and_explicit_keys_without_leaking_saved_keys() {
        let received = Arc::new(std::sync::Mutex::new(Vec::new()));
        let app = axum::Router::new().fallback({
            let received = received.clone();
            move |uri: axum::http::Uri, headers: HeaderMap| {
                let received = received.clone();
                async move {
                    received.lock().unwrap().push((
                        uri.path().to_string(),
                        headers["authorization"].to_str().unwrap().to_string(),
                    ));
                    Json(json!({"data":[{"id":"draft-model"}]}))
                }
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = test_state();
        let mut channel = Channel {
            enabled: true,
            endpoint: endpoint.clone(),
            provider: "openai".into(),
            primary_model: "model".into(),
            api_key: "saved-secret".into(),
            ..Default::default()
        };
        state.store.save(&mut channel).unwrap();
        let response = fetch_upstream_models(
            State(state.clone()),
            Json(UpstreamModelsRequest {
                channel_id: Some(channel.id),
                endpoint: format!("{endpoint}/draft"),
                ..Default::default()
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(received.lock().unwrap().is_empty());
        for (url, secret, expected_path, expected_secret) in [
            (format!("{endpoint}/v1/"), "", "/v1/models", "saved-secret"),
            (
                format!("{endpoint}/draft"),
                "draft-key",
                "/draft/models",
                "draft-key",
            ),
            (endpoint, "explicit-key", "/models", "explicit-key"),
        ] {
            let response = fetch_upstream_models(
                State(state.clone()),
                Json(UpstreamModelsRequest {
                    channel_id: Some(channel.id),
                    endpoint: url,
                    api_key: secret.into(),
                    ..Default::default()
                }),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response_json(response).await["data"][0], "draft-model");
            assert_eq!(
                std::mem::take(&mut *received.lock().unwrap()),
                vec![(
                    expected_path.to_string(),
                    format!("Bearer {expected_secret}")
                )]
            );
        }
        assert_eq!(state.store.list_keys(channel.id).unwrap()[0].attempts, 1);
        server.abort();
    }

    #[tokio::test]
    async fn client_config_roundtrip() {
        let state = test_state();
        let payload = ClientConfigPayload {
            strategy: Some(ROUTE_SMART_QUALITY.to_string()),
            allowed_channels: Some(vec!["test_chan".to_string()]),
            model: Some("claude-3-7-sonnet-latest".to_string()),
            small_model: None,
            allowed_models: Some(vec!["claude-3-7-sonnet-latest".to_string()]),
            ..Default::default()
        };
        let res = update_client_config(
            State(state.clone()),
            Path("hermes".to_string()),
            Json(payload),
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);

        let cfg = get_client_config(&state.store, "hermes");
        assert_eq!(cfg.strategy.as_deref(), Some(ROUTE_SMART_QUALITY));
        assert_eq!(cfg.allowed_channels, Some(vec!["test_chan".to_string()]));
        assert_eq!(
            cfg.allowed_models,
            Some(vec!["claude-3-7-sonnet-latest".to_string()])
        );
    }

    #[tokio::test]
    async fn unknown_clients_are_rejected() {
        let state = test_state();
        let res = update_client_config(
            State(state.clone()),
            Path("evil".to_string()),
            Json(ClientConfigPayload::default()),
        )
        .await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        assert!(state
            .store
            .get_setting("client_config_evil", "")
            .unwrap()
            .is_empty());

        let res = forward_client_chat_completions(
            State(state.clone()),
            Path("evil".to_string()),
            HeaderMap::new(),
            axum::body::Bytes::new(),
        )
        .await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);

        let res = gateway_client_models(
            State(state.clone()),
            Path("evil".to_string()),
            Query(HashMap::new()),
        )
        .await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);

        // Hyphenated / mixed-case spellings of known clients are accepted.
        let res = update_client_config(
            State(state),
            Path("Claude-Code".to_string()),
            Json(ClientConfigPayload::default()),
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[test]
    fn hermes_backup_keeps_the_original_config() {
        let dir = std::env::temp_dir().join(format!(
            "heigate-hermes-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let original = "model:\n  default: \"user-model\"\n";
        std::fs::write(dir.join("config.yaml"), original).unwrap();

        for model in ["first", "second"] {
            apply_hermes_config_in_dir(&dir, "key", "http://127.0.0.1:28888/hermes/v1", model, &[])
                .unwrap();
        }

        assert_eq!(
            std::fs::read_to_string(dir.join("config.yaml.heigate.bak")).unwrap(),
            original
        );
        let saved: serde_yaml_ng::Value =
            serde_yaml_ng::from_str(&std::fs::read_to_string(dir.join("config.yaml")).unwrap())
                .unwrap();
        assert_eq!(saved["model"]["default"].as_str(), Some("second"));
        assert!(!dir.join("config.yaml.heigate.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn hermes_env_permissions_are_restricted_even_when_file_already_exists() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!(
            "heigate-hermes-env-mode-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.yaml"), "model: {}\n").unwrap();
        let env_path = dir.join(".env");
        std::fs::write(&env_path, "EXISTING_SECRET=keep\n").unwrap();
        std::fs::set_permissions(&env_path, std::fs::Permissions::from_mode(0o644)).unwrap();

        apply_hermes_config_in_dir(&dir, "new-key", "http://localhost/hermes/v1", "Auto", &[])
            .unwrap();

        assert_eq!(
            std::fs::metadata(&env_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(std::fs::read_to_string(env_path)
            .unwrap()
            .contains("HERMES_CUSTOM_BEND_API_KEY=new-key"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failed_hermes_env_write_rolls_back_the_config_file() {
        let dir = std::env::temp_dir().join(format!(
            "heigate-hermes-rollback-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let original_config = "model:\n  default: original\n";
        let original_env = "EXISTING_SECRET=keep\n";
        std::fs::write(dir.join("config.yaml"), original_config).unwrap();
        std::fs::write(dir.join(".env"), original_env).unwrap();
        // Block the .env temp-file path so the first atomic write succeeds and
        // the second one fails before replacing the existing environment file.
        std::fs::create_dir(dir.join(".env.heigate.tmp")).unwrap();

        assert!(apply_hermes_config_in_dir(
            &dir,
            "new-key",
            "http://localhost/hermes/v1",
            "Auto",
            &[],
        )
        .is_err());
        assert_eq!(
            std::fs::read_to_string(dir.join("config.yaml")).unwrap(),
            original_config
        );
        assert_eq!(
            std::fs::read_to_string(dir.join(".env")).unwrap(),
            original_env
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    async fn response_json(response: Response) -> Value {
        serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 1 << 20)
                .await
                .unwrap(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn protocol_detection_accepts_root_and_v1_base_urls() {
        let app = axum::Router::new()
            .route(
                "/v1/models",
                axum::routing::get(|| async { Json(json!({"data":[{"id":"gpt-test"}]})) }),
            )
            .route(
                "/v1/chat/completions",
                axum::routing::post(|Json(body): Json<Value>| async move {
                    assert_eq!(body["model"], "gpt-test");
                    Json(json!({"choices":[]}))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        for suffix in ["", "/v1", "/v1/"] {
            let response = detect_protocol(
                State(test_state()),
                Json(DetectProtocolRequest {
                    endpoint: format!("http://{address}{suffix}"),
                    ..Default::default()
                }),
            )
            .await;
            let body = response_json(response).await;
            assert_eq!(body["data"]["supports_openai"], true, "suffix: {suffix}");
            assert_eq!(body["data"]["models"], json!(["gpt-test"]));
        }
        server.abort();
    }

    #[tokio::test]
    async fn protocol_detection_uses_real_models_and_compatible_messages_auth() {
        let app = axum::Router::new()
            .route(
                "/v1/models",
                axum::routing::get(|| async { Json(json!({"data":[{"id":"qwen3.8-27b"}]})) }),
            )
            .route(
                "/v1/chat/completions",
                axum::routing::post(|Json(body): Json<Value>| async move {
                    assert_eq!(body["model"], "qwen3.8-27b");
                    Json(json!({"choices":[]}))
                }),
            )
            .route(
                "/v1/messages",
                axum::routing::post(|headers: HeaderMap, Json(body): Json<Value>| async move {
                    assert_eq!(headers["authorization"], "Bearer test-key");
                    assert_eq!(headers["x-api-key"], "test-key");
                    assert_eq!(headers["anthropic-version"], "2023-06-01");
                    assert_eq!(body["model"], "qwen3.8-27b");
                    assert_eq!(body["max_tokens"], 16);
                    Json(json!({"type":"message","content":[]}))
                }),
            )
            .route(
                "/v1/responses",
                axum::routing::post(|Json(body): Json<Value>| async move {
                    assert_eq!(body["model"], "qwen3.8-27b");
                    assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
                    Json(json!({"output":[]}))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let body = response_json(
            detect_protocol(
                State(test_state()),
                Json(DetectProtocolRequest {
                    endpoint: format!("http://{address}"),
                    api_key: "test-key".into(),
                    ..Default::default()
                }),
            )
            .await,
        )
        .await;
        for protocol in [
            "supports_openai",
            "supports_anthropic",
            "supports_responses",
        ] {
            assert_eq!(body["data"][protocol], true, "{body}");
        }
        server.abort();
    }

    #[tokio::test]
    async fn protocol_detection_selects_responses_only_and_prioritizes_selected_model() {
        let app = axum::Router::new()
            .route(
                "/v1/models",
                axum::routing::get(|| async { Json(json!({"data":[{"id":"unusable-model"}]})) }),
            )
            .route(
                "/v1/responses",
                axum::routing::post(|Json(body): Json<Value>| async move {
                    assert_eq!(body["model"], "senseaudio-s2");
                    Json(json!({"output":[]}))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let body = response_json(
            detect_protocol(
                State(test_state()),
                Json(DetectProtocolRequest {
                    endpoint: format!("http://{address}/v1/"),
                    model: Some("senseaudio-s2".into()),
                    ..Default::default()
                }),
            )
            .await,
        )
        .await;
        assert_eq!(body["data"]["supports_openai"], false);
        assert_eq!(body["data"]["supports_anthropic"], false);
        assert_eq!(body["data"]["supports_responses"], true);
        assert_eq!(body["data"]["detected_protocol"], "openai-responses");
        server.abort();
    }

    #[test]
    fn protocol_detection_distinguishes_model_errors_routes_and_uncertainty() {
        assert_eq!(
            classify_protocol_probe(200, br#"{"object":"response","output":{"type":"message","role":"assistant","content":"pong"}}"#, "responses"),
            Some(true)
        );
        assert_eq!(
            classify_protocol_probe(200, br#"{"output":{"unexpected":true}}"#, "responses"),
            None
        );
        for body in [
            json!({"error":{"code":"model_not_found","message":"Unknown model"}}),
            json!({"message":"model does not exist","code":404}),
            json!({"message":"模型不存在"}),
        ] {
            assert_eq!(
                classify_protocol_probe(404, body.to_string().as_bytes(), "responses"),
                Some(true)
            );
        }
        assert_eq!(
            classify_protocol_probe(
                404,
                br#"{"error":{"message":"endpoint not found"}}"#,
                "messages"
            ),
            Some(false)
        );
        assert_eq!(
            classify_protocol_probe(404, b"Not Found", "responses"),
            Some(false)
        );
        assert_eq!(classify_protocol_probe(405, b"", "responses"), Some(false));
        for status in [401, 403, 429, 500, 502, 504] {
            assert_eq!(
                classify_protocol_probe(status, br#"{"error":"failure"}"#, "responses"),
                None
            );
        }
        assert_eq!(
            classify_protocol_probe(404, br#"{"error":"not found"}"#, "responses"),
            None
        );
        assert_eq!(
            classify_protocol_probe(200, b"<html>login</html>", "responses"),
            None
        );
        assert_eq!(
            classify_protocol_probe(200, br#"{"error":"failure"}"#, "messages"),
            None
        );
    }

    #[tokio::test]
    async fn protocol_detection_connection_failure_does_not_choose_a_protocol() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let body = response_json(
            detect_protocol(
                State(test_state()),
                Json(DetectProtocolRequest {
                    endpoint: format!("http://{address}"),
                    ..Default::default()
                }),
            )
            .await,
        )
        .await;
        assert!(body["data"]["supports_openai"].is_null());
        assert!(body["data"]["supports_anthropic"].is_null());
        assert!(body["data"]["supports_responses"].is_null());
        assert!(body["data"]["detected_protocol"].is_null());
        assert!(body["data"]["message"]
            .as_str()
            .unwrap()
            .contains("连接失败"));
    }

    #[tokio::test]
    async fn draft_probe_sends_the_exact_model_and_key_and_redacts_upstream_errors() {
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let app = axum::Router::new().route(
            "/v1/chat/completions",
            axum::routing::post({
                let calls = calls.clone();
                move |headers: HeaderMap, Json(body): Json<Value>| {
                    let calls = calls.clone();
                    async move {
                        let key = headers["authorization"].to_str().unwrap().to_string();
                        calls.lock().unwrap().push((key.clone(), body));
                        if key == "Bearer invalid-draft-key" {
                            (
                                StatusCode::UNAUTHORIZED,
                                Json(json!({"error":"invalid-draft-key is invalid"})),
                            ).into_response()
                        } else {
                            (
                                [("content-type", "text/event-stream")],
                                "data: {\"choices\":[{\"delta\":{\"content\":\"42\"}}]}\n\ndata: [DONE]\n\n",
                            ).into_response()
                        }
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        for (key, expected) in [
            ("valid-draft-key", "operational"),
            ("invalid-draft-key", "error"),
        ] {
            let response = probe_client(Json(probe::Config {
                provider: "openai".into(),
                api_mode: "chat_completions".into(),
                endpoint: format!("http://{address}/v1"),
                api_key: key.into(),
                primary_model: "draft-model".into(),
                extra_models: vec!["must-not-be-called".into()],
                ..Default::default()
            }))
            .await;
            let body = response_json(response).await;
            assert_eq!(body["data"][0]["status"], expected);
            assert!(body["data"][0]["latency_ms"].is_number());
            assert!(!body.to_string().contains(key));
        }
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, "Bearer valid-draft-key");
        assert_eq!(calls[1].0, "Bearer invalid-draft-key");
        assert!(calls.iter().all(|(_, body)| body["model"] == "draft-model"));
        server.abort();
    }

    #[tokio::test]
    async fn unsupported_client_actions_are_rejected_without_persisting_them() {
        let state = test_state();
        for client in ["codex", "claude-desktop", "claude-code", "generic"] {
            let response = update_client_config(
                State(state.clone()),
                Path(client.into()),
                Json(ClientConfigPayload {
                    apply_local: Some(true),
                    restart: Some(true),
                    ..Default::default()
                }),
            )
            .await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            assert!(get_client_config(&state.store, client)
                .apply_local
                .is_none());
        }
    }

    #[test]
    fn failed_hermes_apply_never_restarts_and_process_errors_are_propagated() {
        let payload = ClientConfigPayload {
            apply_local: Some(true),
            restart: Some(true),
            ..Default::default()
        };
        let result = run_hermes_actions_with(
            &payload,
            || Err("disk full".into()),
            |_| panic!("must not launch after failed apply"),
        );
        assert!(result.unwrap_err().contains("disk full"));
        let result = run_hermes_actions_with(&payload, || Ok(()), |_| Err("exit code 1".into()));
        assert!(result.unwrap_err().contains("exit code 1"));
        let payload = ClientConfigPayload {
            launch_terminal: Some(true),
            ..Default::default()
        };
        let result = run_hermes_actions_with(
            &payload,
            || Ok(()),
            |terminal| {
                assert!(terminal);
                Err("terminal unavailable".into())
            },
        );
        assert!(result.unwrap_err().contains("terminal unavailable"));
    }

    #[test]
    fn command_exit_status_is_checked() {
        let executable = std::env::current_exe().unwrap();
        assert!(checked_command(std::process::Command::new(&executable).arg("--list")).is_ok());
        assert!(checked_command(
            std::process::Command::new(&executable).arg("--heigate-invalid-option")
        )
        .is_err());
    }

    #[test]
    fn hermes_merge_replaces_connection_fields_and_preserves_unrelated_configuration() {
        let original = r#"
model: {default: old, provider: anthropic, base_url: 'https://old.example', key_env: OLD_KEY, api_key: old-secret, temperature: 0.4}
providers:
  other: {base_url: 'https://other.example', custom: keep}
  bend:
    api_key: stale-secret
    timeout: 42
    models: {keep-model: {context_length: 1234}, obsolete: {}}
fallback_providers: [other]
mcp_servers: {filesystem: {command: node, args: [script.js]}}
custom_setting: true
"#;
        let merged = merge_hermes_config(
            original,
            "http://127.0.0.1:28889/hermes/v1",
            "Auto",
            &["keep-model".into(), "model: with spaces".into()],
        )
        .unwrap();
        let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(&merged).unwrap();
        assert_eq!(value["model"]["provider"].as_str(), Some("bend"));
        assert_eq!(value["model"]["default"].as_str(), Some("Auto"));
        assert_eq!(
            value["model"]["key_env"].as_str(),
            Some("HERMES_CUSTOM_BEND_API_KEY")
        );
        assert_eq!(
            value["model"]["api_mode"].as_str(),
            Some("chat_completions")
        );
        assert_eq!(
            value["model"]["base_url"].as_str(),
            Some("http://127.0.0.1:28889/hermes/v1")
        );
        assert!(value["model"].get("api_key").is_none());
        let bend = &value["providers"]["bend"];
        assert!(bend.get("api_key").is_none());
        assert_eq!(bend["base_url"], value["model"]["base_url"]);
        assert_eq!(bend["key_env"], value["model"]["key_env"]);
        assert_eq!(bend["timeout"].as_i64(), Some(42));
        assert_eq!(
            bend["models"]["keep-model"]["context_length"].as_i64(),
            Some(1234)
        );
        assert!(bend["models"].get("Auto").is_some());
        assert!(bend["models"].get("model: with spaces").is_some());
        assert!(bend["models"].get("obsolete").is_none());
        let old: serde_yaml_ng::Value = serde_yaml_ng::from_str(original).unwrap();
        for field in ["mcp_servers", "fallback_providers", "custom_setting"] {
            assert_eq!(value[field], old[field]);
        }
        assert_eq!(value["providers"]["other"], old["providers"]["other"]);
        assert_eq!(value["model"]["temperature"], old["model"]["temperature"]);
    }

    #[test]
    fn hermes_merge_fills_missing_provider_and_supports_scalar_or_empty_configs() {
        for original in [
            "",
            "model: old-model\n",
            "model:\n  default: old\n",
            "model: null\nproviders: null\n",
        ] {
            let merged = merge_hermes_config(
                original,
                "http://127.0.0.1:28888/hermes/v1",
                "gpt-test",
                &[],
            )
            .unwrap();
            let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(&merged).unwrap();
            assert_eq!(value["model"]["provider"].as_str(), Some("bend"));
            assert_eq!(
                value["providers"]["bend"]["model"].as_str(),
                Some("gpt-test")
            );
            assert_eq!(
                value["providers"]["bend"]["api_mode"].as_str(),
                Some("chat_completions")
            );
        }
    }

    #[test]
    fn invalid_hermes_yaml_does_not_change_either_existing_file() {
        let dir = std::env::temp_dir().join(format!(
            "heigate-yaml-invalid-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        for original in ["model: [unterminated", "model: {}\nproviders: []\n"] {
            std::fs::write(dir.join("config.yaml"), original).unwrap();
            std::fs::write(dir.join(".env"), "EXISTING_SECRET=keep\n").unwrap();
            assert!(apply_hermes_config_in_dir(
                &dir,
                "new-key",
                "http://localhost/hermes/v1",
                "Auto",
                &[]
            )
            .is_err());
            assert_eq!(
                std::fs::read_to_string(dir.join("config.yaml")).unwrap(),
                original
            );
            assert_eq!(
                std::fs::read_to_string(dir.join(".env")).unwrap(),
                "EXISTING_SECRET=keep\n"
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn generic_routes_share_model_channel_and_strategy_policy() {
        use tower::ServiceExt;
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let upstream = axum::Router::new().route("/v1/chat/completions",axum::routing::post({
            let calls = calls.clone();
            move |headers: HeaderMap| { let calls = calls.clone(); async move {
                calls.lock().unwrap().push(headers["authorization"].to_str().unwrap().to_string());
                Json(json!({"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}]}))
            }}
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
        let state = test_state();
        for name in ["excluded", "selected"] {
            let mut channel = Channel {
                name: name.into(),
                provider: "openai".into(),
                api_mode: "chat_completions".into(),
                endpoint: format!("http://{address}"),
                api_key: format!("{name}-key"),
                primary_model: "gpt-a".into(),
                extra_models: vec![if name == "selected" {
                    "gpt-b".into()
                } else {
                    "gpt-c".into()
                }],
                enabled: true,
                ..Default::default()
            };
            state.store.save(&mut channel).unwrap();
        }
        let result = update_client_config(
            State(state.clone()),
            Path("generic".into()),
            Json(ClientConfigPayload {
                allowed_channels: Some(vec!["selected".into()]),
                allowed_models: Some(vec!["gpt-a".into()]),
                strategy: Some("manual_priority".into()),
                ..Default::default()
            }),
        )
        .await;
        assert_eq!(result.status(), StatusCode::OK);
        let app = crate::server::router(
            state,
            std::env::temp_dir().join("heigate-missing-test-static-dir"),
        )
        .layer(axum::Extension(axum::extract::ConnectInfo(
            "127.0.0.1:30000".parse::<std::net::SocketAddr>().unwrap(),
        )));
        for prefix in ["/v1", "/generic/v1"] {
            let request = axum::http::Request::builder()
                .uri(format!("{prefix}/models"))
                .header("host", "127.0.0.1")
                .header("authorization", "Bearer test")
                .body(Body::empty())
                .unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let models = response_json(response).await;
            assert_eq!(models["data"].as_array().unwrap().len(), 1);
            assert_eq!(models["data"][0]["id"], "gpt-a");
            for endpoint in ["messages", "chat/completions", "responses"] {
                let request = axum::http::Request::builder()
                    .method("POST")
                    .uri(format!("{prefix}/{endpoint}"))
                    .header("host", "127.0.0.1")
                    .header("authorization", "Bearer test")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"model":"gpt-b","messages":[{"role":"user","content":"hi"}]}"#,
                    ))
                    .unwrap();
                let response = app.clone().oneshot(request).await.unwrap();
                assert_eq!(
                    response.status(),
                    StatusCode::FORBIDDEN,
                    "{prefix}/{endpoint}"
                );
            }
            let request = axum::http::Request::builder()
                .method("POST")
                .uri(format!("{prefix}/chat/completions"))
                .header("host", "127.0.0.1")
                .header("authorization", "Bearer test")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"model":"gpt-a","messages":[{"role":"user","content":"hi"}]}"#,
                ))
                .unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let _ = response_json(response).await;
        }
        assert_eq!(
            *calls.lock().unwrap(),
            ["Bearer selected-key", "Bearer selected-key"]
        );
        server.abort();
    }

    #[tokio::test]
    async fn dropping_log_stream_releases_subscription_before_and_after_polling() {
        use futures_util::StreamExt;
        let state = test_state();
        for _ in 0..5 {
            let response = stream_logs(State(state.clone())).await;
            assert_eq!(state.store.log_subscriber_count(), 1);
            drop(response);
            assert_eq!(state.store.log_subscriber_count(), 0);
            let response = stream_logs(State(state.clone())).await;
            let mut body = response.into_body().into_data_stream();
            assert!(body
                .next()
                .await
                .unwrap()
                .unwrap()
                .starts_with(b"event: connected"));
            assert_eq!(state.store.log_subscriber_count(), 1);
            drop(body);
            assert_eq!(state.store.log_subscriber_count(), 0);
        }
    }

    #[tokio::test]
    async fn model_listing_and_forwarding_share_auto_allowlist_and_fallback() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = axum::Router::new().route(
            "/v1/chat/completions",
            axum::routing::post(|| async {
                Json(json!({"choices":[{"message":{"role":"assistant","content":"ok"}}]}))
            }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = test_state();
        let mut channel = Channel {
            name: "test".into(),
            provider: "openai".into(),
            api_mode: "chat_completions".into(),
            endpoint: format!("http://{addr}"),
            api_key: "test".into(),
            primary_model: "model-A".into(),
            extra_models: vec!["model-B".into()],
            enabled: true,
            ..Default::default()
        };
        state.store.save(&mut channel).unwrap();
        for (enabled, auto_models, expected, denied) in [
            (true, vec!["model-B"], "model-B", "model-A"),
            (true, vec![], "model-A", "model-B"),
            (false, vec!["model-B"], "model-A", "model-B"),
        ] {
            for client in ["generic", "hermes"] {
                state.store.set_setting(&format!("client_config_{client}"), &json!({"auto_enabled":enabled,"auto_models":auto_models,"allowed_models":["model-A"]}).to_string()).unwrap();
                let response = gateway_client_models(
                    State(state.clone()),
                    Path(client.into()),
                    Query(HashMap::from([("raw".into(), "1".into())])),
                )
                .await;
                let bytes = axum::body::to_bytes(response.into_body(), 10000)
                    .await
                    .unwrap();
                let payload: Value = serde_json::from_slice(&bytes).unwrap();
                let listed = payload["data"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|m| m["id"].as_str().unwrap())
                    .filter(|m| *m != "Auto")
                    .collect::<Vec<_>>();
                assert_eq!(listed, vec![expected]);
                for (model, status) in [(expected, StatusCode::OK), (denied, StatusCode::FORBIDDEN)]
                {
                    let body = Bytes::from(
                        json!({"model":model,"messages":[{"role":"user","content":"hi"}]})
                            .to_string(),
                    );
                    let response = if client == "generic" {
                        forward_chat_completions(State(state.clone()), HeaderMap::new(), body).await
                    } else {
                        forward_client_chat_completions(
                            State(state.clone()),
                            Path(client.into()),
                            HeaderMap::new(),
                            body,
                        )
                        .await
                    };
                    assert_eq!(response.status(), status);
                    axum::body::to_bytes(response.into_body(), 10000)
                        .await
                        .unwrap();
                }
            }
        }
        server.abort();
    }
}
