//! The local management API (`/api/*`). Ported from `internal/api/handler.go`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use axum::Json;
use futures_util::StreamExt;
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
    if response
        .content_length()
        .is_some_and(|length| length > MAX_UPSTREAM_RESPONSE_BODY as u64)
    {
        return Err(());
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ())?;
        if body.len().saturating_add(chunk.len()) > MAX_UPSTREAM_RESPONSE_BODY {
            return Err(());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
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
    if input.api_key.trim().is_empty() || input.api_key.contains("***") {
        input.api_key = existing.api_key.clone();
    }

    let incoming = match request_to_channel(input) {
        Ok(channel) => channel,
        Err(err) => return probe_error_response(&err),
    };

    let mut updated: Option<Channel> = None;
    let result = state.store.update_runtime(id, |fresh| {
        if fresh.interval_seconds != incoming.interval_seconds
            || fresh.primary_model != incoming.primary_model
        {
            fresh.probe_streak = 0;
        }
        copy_config_state(fresh, &incoming);
        updated = Some(fresh.clone());
    });
    match result {
        Ok(()) => {
            let updated = updated.unwrap_or(existing);
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
        let lease = match state.gateway.key_pool.acquire_for_probe(id, model, None) {
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
            key_origins.push((lease.key.id, lease.key.name.clone()));
        }
        reports.extend(result);
    }

    // The channel column reflects the primary model.
    if let Some(report) = reports.iter().find(|r| r.model == channel.primary_model) {
        if let Err(err) =
            state
                .store
                .set_injection(id, &report.state, report.prompt_tokens, &report.content)
        {
            return store_error_response(&err);
        }
    }

    let reports: Vec<_> = reports
        .iter()
        .zip(key_origins)
        .map(|(report, (id, name))| {
            let mut value = json!(report);
            value["key_id"] = json!(id);
            value["key_name"] = json!(name);
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
        let lease =
            match state
                .gateway
                .key_pool
                .acquire(id, "", &std::collections::HashSet::new(), None)
            {
                Ok(Some(lease)) => lease,
                _ => {
                    return json_response(
                        StatusCode::CONFLICT,
                        json!({"error":"没有可用于获取模型的 Key"}),
                    )
                }
            };
        req.endpoint = channel.endpoint;
        req.provider = channel.provider;
        req.api_key = lease.key.secret.clone();
        Some(lease)
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
}

/// Automatically tests the endpoint for OpenAI and Anthropic protocol support.
pub async fn detect_protocol(
    State(state): State<SharedState>,
    Json(mut req): Json<DetectProtocolRequest>,
) -> Response {
    if let Some(id) = req.channel_id {
        if let Ok(c) = state.store.get(id) {
            req.endpoint = c.endpoint;
            if let Ok(Some(lease)) =
                state
                    .gateway
                    .key_pool
                    .acquire(id, "", &std::collections::HashSet::new(), None)
            {
                req.api_key = lease.key.secret.clone();
            }
        }
    }

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
        .timeout(Duration::from_secs(8))
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
    let url_lower = endpoint.to_lowercase();
    let is_anthropic_domain = url_lower.contains("anthropic.com");

    let mut supports_openai = false;
    let mut supports_anthropic = false;
    let mut models: Vec<String> = Vec::new();
    let mut openai_msg = String::new();
    let mut anthropic_msg = String::new();

    // 1. Probe OpenAI endpoint (/v1/models and /models)
    let candidate_openai_urls = if endpoint.ends_with("/models") {
        vec![endpoint.clone()]
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
            let status = res.status().as_u16();
            if let Ok(body) = res.bytes().await {
                if status == 200 {
                    let parsed = parse_model_list(&body);
                    if !parsed.is_empty() {
                        models = parsed;
                    }
                    supports_openai = true;
                    openai_msg = "成功获取 OpenAI 模型列表".to_string();
                    break;
                } else if status == 401 || status == 400 || status == 403 {
                    if let Ok(val) = serde_json::from_slice::<Value>(&body) {
                        if val.get("error").is_some() {
                            supports_openai = true;
                            openai_msg = format!("端点响应 OpenAI 规范 (HTTP {status})");
                            break;
                        }
                    }
                }
            }
        }
    }

    // 2. Probe Anthropic endpoint (/v1/messages)
    let anthropic_target = if endpoint.ends_with("/v1") {
        format!("{endpoint}/messages")
    } else {
        format!("{endpoint}/v1/messages")
    };
    let mut ant_request = client
        .post(&anthropic_target)
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .header("anthropic-version", "2023-06-01")
        .header("user-agent", "HeiGate/1.0")
        .json(&serde_json::json!({
            "model": "claude-3-7-sonnet-latest",
            "messages": [{"role": "user", "content": "ping"}],
            "max_tokens": 1
        }));
    if !api_key.is_empty() && !api_key.contains("***") {
        ant_request = ant_request.header("x-api-key", api_key);
    }
    if let Ok(res) = ant_request.send().await {
        let status = res.status().as_u16();
        if let Ok(body) = res.bytes().await {
            if (200..300).contains(&status) {
                supports_anthropic = true;
                anthropic_msg = "Anthropic 消息接口响应正常 (HTTP 200)".to_string();
            } else if let Ok(val) = serde_json::from_slice::<Value>(&body) {
                if val.get("type").and_then(Value::as_str) == Some("error")
                    || val.get("error").is_some()
                {
                    supports_anthropic = true;
                    anthropic_msg = format!("端点识别为 Anthropic 协议 (HTTP {status})");
                }
            }
        }
    }

    // 3. Probe Responses endpoint (/v1/responses)
    let mut supports_responses = false;
    let responses_target = if endpoint.ends_with("/v1") {
        format!("{endpoint}/responses")
    } else {
        format!("{endpoint}/v1/responses")
    };
    let mut resp_request = client
        .post(&responses_target)
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .header("user-agent", "HeiGate/1.0")
        .json(&serde_json::json!({
            "model": "gpt-4o",
            "input": "ping"
        }));
    if !api_key.is_empty() && !api_key.contains("***") {
        resp_request = resp_request.header("authorization", format!("Bearer {api_key}"));
    }
    if let Ok(res) = resp_request.send().await {
        let status = res.status().as_u16();
        if status != 404 && status != 405 && status != 502 && status != 504 {
            if let Ok(body) = res.bytes().await {
                if let Ok(val) = serde_json::from_slice::<Value>(&body) {
                    if val.get("id").is_some()
                        || val.get("output").is_some()
                        || val.get("error").is_some()
                    {
                        supports_responses = true;
                    }
                }
            }
        }
    }

    let detected_protocol = if is_anthropic_domain || (supports_anthropic && !supports_openai) {
        "anthropic"
    } else if supports_openai {
        "openai-compatible"
    } else if supports_anthropic {
        "anthropic"
    } else {
        if is_anthropic_domain {
            "anthropic"
        } else {
            "openai-compatible"
        }
    };

    let summary = if supports_openai && supports_anthropic {
        "端点同时支持 OpenAI 兼容与 Anthropic 协议，已为您选中主流推荐".to_string()
    } else if detected_protocol == "anthropic" {
        if !anthropic_msg.is_empty() {
            format!("已自动识别为 Anthropic 协议（{anthropic_msg}）")
        } else {
            "已根据端点特征自动识别为 Anthropic 原生协议".to_string()
        }
    } else if supports_openai {
        if !openai_msg.is_empty() {
            format!("已自动识别为 OpenAI 兼容协议（{openai_msg}）")
        } else {
            "已自动识别为 OpenAI 兼容协议".to_string()
        }
    } else {
        "未识别到专属特征，已默认设为 OpenAI 兼容协议".to_string()
    };

    ok_data(json!({
        "detected_protocol": detected_protocol,
        "supports_openai": supports_openai,
        "supports_anthropic": supports_anthropic,
        "supports_responses": supports_responses,
        "models": models,
        "message": summary
    }))
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

fn yaml_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
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
    let quoted_model = yaml_string(model);
    let quoted_url = yaml_string(base_url);

    if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)?;
        let backup_path = hermes_dir.join("config.yaml.heigate.bak");
        // Keep the user's original config: later applies must not overwrite a
        // backup that already exists with an already-modified copy.
        if !backup_path.exists() {
            std::fs::write(&backup_path, &content)?;
        }

        let lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
        let mut in_models_block = false;
        let mut in_bend_provider = false;
        let mut current_section = String::new();
        let mut updated_lines = Vec::new();

        for line in lines {
            let trimmed = line.trim();
            if !line.starts_with(' ') && !line.starts_with('\t') && trimmed.ends_with(':') {
                current_section = trimmed.trim_end_matches(':').to_string();
                in_bend_provider = false;
            } else if current_section == "providers"
                && line.starts_with("  ")
                && !line.starts_with("    ")
                && trimmed.ends_with(':')
            {
                in_bend_provider = trimmed == "bend:";
            }

            // Update base_url
            if trimmed.starts_with("base_url:") && (current_section == "model" || in_bend_provider)
            {
                let indent = line.len() - line.trim_start().len();
                let prefix = " ".repeat(indent);
                updated_lines.push(format!("{prefix}base_url: {quoted_url}"));
                continue;
            }
            // Update default under top-level model:
            if current_section == "model" && trimmed.starts_with("default:") && !in_models_block {
                let indent = line.len() - line.trim_start().len();
                let prefix = " ".repeat(indent);
                updated_lines.push(format!("{prefix}default: {quoted_model}"));
                continue;
            }
            // Update model under providers.bend:
            if in_bend_provider && trimmed.starts_with("model:") && line.starts_with("    model:") {
                updated_lines.push(format!("    model: {quoted_model}"));
                continue;
            }
            // Update models dictionary
            if in_bend_provider && trimmed == "models:" && line.starts_with("    models:") {
                in_models_block = true;
                updated_lines.push("    models:".to_string());
                if allowed_models.is_empty() {
                    updated_lines.push(format!("      {}: {{}}", yaml_string(model)));
                } else {
                    for m in allowed_models {
                        updated_lines.push(format!("      {}: {{}}", yaml_string(m)));
                    }
                }
                continue;
            }
            if in_models_block {
                if line.starts_with("      ") || (trimmed.is_empty() && in_models_block) {
                    continue;
                } else {
                    in_models_block = false;
                }
            }
            updated_lines.push(line);
        }

        let mut final_content = updated_lines.join("\n");
        if !final_content.ends_with('\n') {
            final_content.push('\n');
        }
        write_atomic(&config_path, &final_content)?;
    } else {
        let models = if allowed_models.is_empty() {
            vec![model]
        } else {
            allowed_models.iter().map(String::as_str).collect()
        };
        let model_entries = models
            .into_iter()
            .map(|name| format!("      {}: {{}}", yaml_string(name)))
            .collect::<Vec<_>>()
            .join("\n");
        let content = format!(
            "model:\n  default: {quoted_model}\n  provider: bend\n  base_url: {quoted_url}\n  key_env: HERMES_CUSTOM_BEND_API_KEY\n  api_mode: chat_completions\nproviders:\n  bend:\n    name: HeiGate\n    base_url: {quoted_url}\n    model: {quoted_model}\n    discover_models: true\n    models:\n{model_entries}\n    key_env: HERMES_CUSTOM_BEND_API_KEY\nfallback_providers: []\n"
        );
        write_atomic(&config_path, &content)?;
    }

    {
        let existed = env_path.exists();
        let env_content = if existed {
            std::fs::read_to_string(&env_path)?
        } else {
            String::new()
        };
        let key_line = format!("HERMES_CUSTOM_BEND_API_KEY={gateway_key}");
        let re_key = regex::Regex::new(r"(?m)^HERMES_CUSTOM_BEND_API_KEY=.*$").unwrap();
        let new_env = if re_key.is_match(&env_content) {
            re_key.replace_all(&env_content, &key_line).to_string()
        } else if env_content.is_empty() {
            format!("{key_line}\n")
        } else {
            format!("{}\n{key_line}\n", env_content.trim_end_matches('\n'))
        };
        write_atomic(&env_path, &new_env)?;
        #[cfg(unix)]
        if !existed {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&env_path, std::fs::Permissions::from_mode(0o600))?;
        }
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

/// Applies the local side effects of saving the Hermes client config (config
/// rewrite, gateway restart, terminal launch). Blocking by nature — file I/O
/// and child processes — so callers must run it via `spawn_blocking`.
fn run_hermes_actions(
    payload: &ClientConfigPayload,
    gateway_key: &str,
    gateway_port: u16,
) -> String {
    let mut msg = String::new();
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
        match apply_hermes_local_config(gateway_key, gateway_port, m, am) {
            Ok(_) => msg.push_str("已更新 Hermes 本地配置 (~/.hermes/config.yaml & .env)。"),
            Err(e) => msg.push_str(&format!("更新本地配置失败: {e}。")),
        }
    }
    if payload.restart.unwrap_or(false) {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let hermes_bin = std::path::PathBuf::from(&home).join(".local/bin/hermes");
        let bin = if hermes_bin.exists() {
            hermes_bin.to_string_lossy().to_string()
        } else {
            "hermes".to_string()
        };
        match std::process::Command::new(bin)
            .arg("gateway")
            .arg("restart")
            .output()
        {
            Ok(out) => {
                if out.status.success() {
                    msg.push_str(" 已成功重启 Hermes 网关服务！");
                } else {
                    let err_str = String::from_utf8_lossy(&out.stderr);
                    msg.push_str(&format!(" Hermes 重启响应: {}", err_str.trim()));
                }
            }
            Err(e) => msg.push_str(&format!(" 执行重启命令失败: {e}")),
        }
    }
    if payload.launch_terminal.unwrap_or(false) {
        let _ = std::process::Command::new("osascript")
            .arg("-e")
            .arg("tell application \"Terminal\" to do script \"hermes\"")
            .output();
        msg.push_str(" 已在终端启动 Hermes 对话。");
    }
    msg.trim().to_string()
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
        .unwrap_or_else(|err| format!("执行 Hermes 操作失败: {err}"));
        return ok_data(json!({ "status": "ok", "client_id": client_id, "message": msg }));
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
        let models = if cfg.auto_enabled.unwrap_or(false)
            && cfg
                .auto_models
                .as_ref()
                .map(|m| !m.is_empty())
                .unwrap_or(false)
        {
            cfg.auto_models
        } else {
            cfg.allowed_models
        };
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
    forward_with_proto_and_client(state, proto, headers, path, body, None).await
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
    let channels = match state.store.list() {
        Ok(channels) => channels,
        Err(err) => return store_error_response(&err),
    };
    let cfg = get_client_config(&state.store, "generic");
    build_gateway_models_response(
        &channels,
        &params,
        cfg.allowed_models.as_deref(),
        cfg.auto_enabled.unwrap_or(false),
    )
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
    let filtered_channels: Vec<_> = if let Some(allowed) = cfg.allowed_channels {
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
    build_gateway_models_response(
        &filtered_channels,
        &params,
        cfg.allowed_models.as_deref(),
        auto_on,
    )
}

/// Streams request-log events as SSE.
pub async fn stream_logs(State(state): State<SharedState>) -> Response {
    let (receiver, sub_id) = state.store.subscribe_logs();
    let store = state.store.clone();

    let stream = futures_util::stream::unfold(
        (receiver, sub_id, store, false),
        |(mut receiver, sub_id, store, sent_connected)| async move {
            if !sent_connected {
                let frame = "event: connected\ndata: {\"connected\":true}\n\n".to_string();
                return Some((
                    Ok::<_, std::convert::Infallible>(Bytes::from(frame)),
                    (receiver, sub_id, store, true),
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
                            Some((Ok(Bytes::from(frame)), (receiver, sub_id, store, true)))
                        }
                        None => None,
                    }
                }
                _ = ticker.tick() => {
                    let frame = format!(
                        "event: ping\ndata: {{\"ts\":{}}}\n\n",
                        chrono::Utc::now().timestamp_millis()
                    );
                    Some((Ok(Bytes::from(frame)), (receiver, sub_id, store, true)))
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
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
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
        assert!(std::fs::read_to_string(dir.join("config.yaml"))
            .unwrap()
            .contains("\"second\""));
        assert!(!dir.join("config.yaml.heigate.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
