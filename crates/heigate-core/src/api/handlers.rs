//! The local management API (`/api/*`). Ported from `internal/api/handler.go`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::gateway::{Gateway, RequestContext};
use crate::api::policy::AUTO_MODEL_NAME;
use crate::canonical::{
    get_canonical_group_key, get_custom_model_mappings, select_canonical_display_name,
    set_custom_model_mappings,
};
use crate::probe::{
    self, check_injection_models, validate_config, validate_interval, ProbeError,
    API_MODE_CHAT_COMPLETIONS,
};
use crate::router::{Router, ROUTE_LATENCY, ROUTE_ROUND_ROBIN, ROUTE_SMART_QUALITY};
use crate::runner::{ProbeRunner, RunnerError, ERR_CHANNEL_DISABLED, ERR_PROBE_BUSY};
use crate::store::{copy_config_state, Channel, Store, StoreError};

/// Shared application state for the management API and the gateway.
pub struct AppState {
    pub store: Arc<Store>,
    pub runner: Arc<ProbeRunner>,
    pub router: Arc<Router>,
    pub gateway: Arc<Gateway>,
    pub gateway_key: String,
    pub version: String,
}

pub type SharedState = Arc<AppState>;

fn ok_data(data: Value) -> Response {
    json_response(StatusCode::OK, json!({ "data": data }))
}

fn json_response(status: StatusCode, body: Value) -> Response {
    let mut response = Response::new(Body::from(body.to_string()));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert("content-type", HeaderValue::from_static("application/json"));
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
    copy.api_key = if copy.api_key.len() > 4 {
        format!("{}***", &copy.api_key[..4])
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
        let lease =
            match state
                .gateway
                .key_pool
                .acquire(id, model, &std::collections::HashSet::new(), None)
            {
                Ok(Some(lease)) => lease,
                _ => {
                    return json_response(
                        StatusCode::CONFLICT,
                        json!({"error":"没有可用于检测的 Key（冷却、禁用或权限限制）"}),
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
            if report.message.contains("429") {
                let _ = lease.failure(&crate::resilience::classify_status(429), model);
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
    let mut has_enabled = false;
    for ch in &channels {
        if !ch.enabled {
            continue;
        }
        has_enabled = true;
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
    if has_enabled {
        data.push(json!({ "name": AUTO_MODEL_NAME, "channels": ["HeiGate 自动路由"] }));
    }
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
    set_custom_model_mappings(mappings.clone());
    let _ = state.store.set_setting(
        "custom_model_mappings",
        &serde_json::to_string(&mappings).unwrap_or_default(),
    );
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
    set_custom_model_mappings(mappings.clone());
    let _ = state.store.set_setting(
        "custom_model_mappings",
        &serde_json::to_string(&mappings).unwrap_or_default(),
    );
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

    ok_data(json!({
        "data_dir": crate::datadir::path().to_string_lossy(),
        "db_path": db_path.to_string_lossy(),
        "db_size_bytes": db_size,
        "log_retention_days": retention_days,
        "log_max_count": max_count,
        "logs_count": logs_count,
        "routing_strategy": route_strategy(&state.store, &HashMap::new()),
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
}

pub async fn update_settings(
    State(state): State<SharedState>,
    Json(input): Json<SettingsRequest>,
) -> Response {
    let mut changed = false;
    if let Some(days) = input.log_retention_days {
        if let Err(err) = state
            .store
            .set_setting("log_retention_days", &days.to_string())
        {
            return store_error_response(&err);
        }
        changed = true;
    }
    if let Some(max_count) = input.log_max_count {
        if let Err(err) = state
            .store
            .set_setting("log_max_count", &max_count.to_string())
        {
            return store_error_response(&err);
        }
        changed = true;
    }
    if changed {
        let _ = state.store.auto_prune_logs();
    }
    if let Some(strategy) = input.routing_strategy {
        let strategy = strategy.trim().to_string();
        if strategy != ROUTE_SMART_QUALITY
            && strategy != ROUTE_ROUND_ROBIN
            && strategy != ROUTE_LATENCY
        {
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "error": "invalid routing strategy" }),
            );
        }
        if let Err(err) = state.store.set_setting("routing_strategy", &strategy) {
            return store_error_response(&err);
        }
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
        Err(err) => {
            return json_response(StatusCode::BAD_GATEWAY, json!({ "error": err.to_string() }))
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
            Err(err) => {
                last_err = Some(err.to_string());
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
        let body = response.bytes().await.unwrap_or_default();

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

/// Shared gateway forwarding entry point used by the three `/v1/*` routes.
async fn forward_with_proto(
    state: &AppState,
    proto: crate::api::forward::GatewayProtocol,
    headers: &HeaderMap,
    path: String,
    body: axum::body::Bytes,
) -> Response {
    let ctx = RequestContext {
        client_ip: String::new(),
        user_agent: headers
            .get("user-agent")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string(),
        request_path: path,
        strategy: route_strategy(&state.store, &HashMap::new()),
    };
    state
        .gateway
        .forward(proto, headers, body.to_vec(), &ctx)
        .await
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

/// Handles `GET /v1/models`.
pub async fn gateway_models(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let channels = match state.store.list() {
        Ok(channels) => channels,
        Err(err) => return store_error_response(&err),
    };

    let raw = matches!(
        params.get("raw").map(String::as_str),
        Some("true") | Some("1")
    );
    if raw {
        let mut seen = std::collections::HashSet::new();
        let mut data: Vec<Value> = Vec::new();
        let has_enabled = channels.iter().any(|c| c.enabled);
        if has_enabled {
            data.push(json!({ "id": AUTO_MODEL_NAME, "object": "model", "owned_by": "heigate" }));
        }
        for ch in &channels {
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
        return json_response(StatusCode::OK, json!({ "object": "list", "data": data }));
    }

    // Canonical aggregated models.
    let mut grouped: Vec<(String, Vec<String>)> = Vec::new();
    for ch in &channels {
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
    if !grouped.is_empty() {
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
    data.sort_by(|a, b| {
        a["id"]
            .as_str()
            .unwrap_or("")
            .cmp(b["id"].as_str().unwrap_or(""))
    });
    json_response(StatusCode::OK, json!({ "object": "list", "data": data }))
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
