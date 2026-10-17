//! The gateway endpoints' forwarding core: candidate selection, the first-byte
//! failover loop, protocol translation, and the client-facing stream with
//! request-log bookkeeping. Ported from `internal/api/gateway.go`.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use futures_util::stream::{self, Stream, StreamExt};
use serde_json::json;

use crate::api::forward::{
    anthropic_error_type, estimate_prompt_tokens, gateway_candidate_matcher, is_hop_by_hop_header,
    parse_retry_after, request_model, GatewayProtocol, StreamTokenTracker, StreamTtftTracker,
};
use crate::api::policy::{
    adapt_unsupported_reasoning_effort, auto_choice_for_strategy, auto_choices_for_strategy,
    channel_serving_traffic, channel_supports_model, parse_upstream_error_message,
    upstream_error_payload, upstream_stream_error, TraceStep, AUTO_MODEL_NAME,
};
use crate::canonical::{
    are_models_equivalent, channel_supports_model_or_canonical, find_channel_model_for_request,
};
use crate::key_pool::{KeyLease, KeyPool};
use crate::probe::{normalize_endpoint, PROVIDER_OPENAI};
use crate::protocol::{
    chat_to_messages, encode_sse, messages_to_chat, ChatChunk, MessagesRequest, StreamState,
};
use crate::resilience::{
    classify_status, FailureClass, FailureInfo, FailureInfoProvider, CIRCUIT_HALF_OPEN,
};
use crate::router::Router;
use crate::store::{Channel, RequestLog, Store};

/// The maximum request/response body HeiGate will buffer.
pub const MAX_REQUEST_BODY: usize = 16 << 20;
const MAX_STREAM_PREFIX_BYTES: usize = 64 << 10;
const LOG_UPDATE_INTERVAL: Duration = Duration::from_millis(200);

#[cfg(test)]
#[path = "gateway_key_tests.rs"]
mod key_tests;

#[cfg(test)]
#[path = "gateway_protocol_tests.rs"]
mod protocol_tests;

#[cfg(test)]
#[path = "gateway_regression_tests.rs"]
mod regression_tests;

/// A streaming byte source for an upstream response (or a synthesized body).
pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>>;

/// An upstream reply before it is handed to the client.
pub struct UpstreamRaw {
    pub status: u16,
    pub headers: HeaderMap,
    pub stream: ByteStream,
    ttft: Option<Arc<Mutex<TtftMeasurement>>>,
    expected_choices: usize,
    non_stream_usage_authoritative: bool,
}

struct TtftMeasurement {
    started: Instant,
    started_at: chrono::DateTime<chrono::Utc>,
    tracker: StreamTtftTracker,
}

/// A gateway error, classified so the router can bench or clear a channel.
#[derive(Debug)]
pub enum GatewayError {
    UpstreamStatus {
        status: i32,
        retry_after: Duration,
        message: String,
    },
    Transport {
        message: String,
        client_closed: bool,
    },
    StreamStart {
        message: String,
    },
    Client(String),
    Other(String),
}

impl GatewayError {
    fn redact(&mut self, secret: &str) {
        if secret.is_empty() {
            return;
        }
        let message = match self {
            Self::UpstreamStatus { message, .. }
            | Self::Transport { message, .. }
            | Self::StreamStart { message }
            | Self::Client(message)
            | Self::Other(message) => message,
        };
        *message = message.replace(secret, "[REDACTED]");
    }
    fn upstream_status(status: i32, message: String) -> Self {
        GatewayError::UpstreamStatus {
            status,
            retry_after: Duration::ZERO,
            message,
        }
    }
}

impl std::fmt::Display for GatewayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GatewayError::UpstreamStatus {
                status, message, ..
            } => {
                if message.is_empty() {
                    write!(f, "upstream returned HTTP {status}")
                } else {
                    write!(f, "upstream returned HTTP {status}: {message}")
                }
            }
            GatewayError::Transport { .. } => write!(f, "upstream request failed"),
            GatewayError::StreamStart { message } => {
                write!(f, "upstream stream failed before first byte: {message}")
            }
            GatewayError::Client(message) | GatewayError::Other(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for GatewayError {}

impl FailureInfoProvider for GatewayError {
    fn failure_info(&self) -> FailureInfo {
        match self {
            GatewayError::UpstreamStatus {
                status,
                retry_after,
                ..
            } => {
                let mut info = classify_status(*status);
                info.retry_after = *retry_after;
                info
            }
            GatewayError::Transport { client_closed, .. } => FailureInfo::new(if *client_closed {
                FailureClass::Client
            } else {
                FailureClass::Network
            }),
            GatewayError::StreamStart { .. } | GatewayError::Other(_) => {
                FailureInfo::new(FailureClass::Network)
            }
            GatewayError::Client(_) => classify_status(400),
        }
    }
}

/// Per-request context the handler passes through.
#[derive(Debug, Clone, Default)]
pub struct RequestContext {
    pub client_ip: String,
    pub user_agent: String,
    pub request_path: String,
    pub strategy: String,
    pub allowed_channels: Option<Vec<String>>,
    pub allowed_models: Option<Vec<String>>,
}

/// The gateway: candidate routing plus upstream forwarding.
pub struct Gateway {
    pub store: Arc<Store>,
    pub router: Arc<Router>,
    pub key_pool: Arc<KeyPool>,
    client: reqwest::Client,
}

struct RecoveryClaims {
    router: Arc<Router>,
    ids: Vec<i64>,
}
impl Drop for RecoveryClaims {
    fn drop(&mut self) {
        for id in &self.ids {
            self.router.release_half_open_public(*id);
        }
    }
}

impl Gateway {
    pub fn new(store: Arc<Store>, router: Arc<Router>) -> Self {
        Gateway {
            key_pool: KeyPool::new(store.clone()),
            store,
            router,
            client: reqwest::Client::builder()
                .redirect(crate::probe::upstream_redirect_policy())
                .timeout(Duration::from_secs(600))
                .build()
                .expect("build upstream client"),
        }
    }

    /// Handles one gateway request and returns the client response.
    pub async fn forward(
        &self,
        proto: GatewayProtocol,
        inbound: &HeaderMap,
        body: Vec<u8>,
        ctx: &RequestContext,
    ) -> Response {
        let (model, stream_requested) = match request_model(&body) {
            Ok(parsed) => parsed,
            Err(message) => return json_error(StatusCode::BAD_REQUEST, &message),
        };
        if proto == GatewayProtocol::Anthropic {
            let validation = serde_json::from_slice::<MessagesRequest>(&body)
                .map_err(|err| format!("invalid anthropic request: {err}"))
                .and_then(|request| {
                    messages_to_chat(Some(&request)).map_err(|err| err.to_string())
                });
            if let Err(message) = validation {
                return anthropic_error(StatusCode::BAD_REQUEST, "invalid_request_error", &message);
            }
        }

        let is_auto = model.eq_ignore_ascii_case(AUTO_MODEL_NAME);
        let mut effective_model = self.resolve_model_alias(&model);
        if is_auto {
            effective_model = AUTO_MODEL_NAME.to_string();
        }
        let candidate_model = if is_auto {
            String::new()
        } else {
            effective_model.clone()
        };

        if let Some(ref allowed_models) = ctx.allowed_models {
            if !allowed_models.is_empty() && !is_auto {
                let is_allowed = allowed_models.iter().any(|m| {
                    are_models_equivalent(m, &model) || are_models_equivalent(m, &effective_model)
                });
                if !is_allowed {
                    let err_msg = format!(
                        "Model '{model}' is not allowed for this client. Allowed models: {}",
                        allowed_models.join(", ")
                    );
                    if proto == GatewayProtocol::Anthropic {
                        return anthropic_error(
                            StatusCode::FORBIDDEN,
                            "invalid_request_error",
                            &err_msg,
                        );
                    } else {
                        return json_error(StatusCode::FORBIDDEN, &err_msg);
                    }
                }
            }
        }

        let allowed_channels = ctx.allowed_channels.clone();
        let allowed_models = ctx.allowed_models.clone();
        let matcher = move |ch: &Channel| {
            if let Some(ref allowed) = allowed_channels {
                if !allowed.is_empty() && !allowed.contains(&ch.name) {
                    return false;
                }
            }
            if is_auto {
                if let Some(ref allowed_m) = allowed_models {
                    if !allowed_m.is_empty() {
                        let has_any = allowed_m
                            .iter()
                            .any(|m| channel_supports_model_or_canonical(ch, m));
                        if !has_any {
                            return false;
                        }
                    }
                }
            }
            gateway_candidate_matcher(proto, ch)
        };
        let mut candidates = match self.router.candidates_matching(
            &candidate_model,
            &ctx.strategy,
            Some(&matcher),
        ) {
            Ok(candidates) => candidates,
            Err(err) => {
                if proto == GatewayProtocol::Anthropic {
                    return anthropic_error(
                            StatusCode::SERVICE_UNAVAILABLE,
                            "api_error",
                            &format!(
                                "No channel found for model {model:?} (resolved: {effective_model:?}): {err}"
                            ),
                        );
                }
                return json_error(StatusCode::SERVICE_UNAVAILABLE, &err);
            }
        };

        if is_auto {
            let allowed_m = ctx.allowed_models.as_deref();
            candidates.sort_by(|a, b| {
                // Stable sorting preserves the router's rotation within each safety tier.
                let safety = crate::router::candidate_safety_priority(a)
                    .cmp(&crate::router::candidate_safety_priority(b));
                if !safety.is_eq() {
                    return safety;
                }
                let ca = auto_choice_for_strategy(Some(a), allowed_m, &ctx.strategy);
                let cb = auto_choice_for_strategy(Some(b), allowed_m, &ctx.strategy);
                match ctx.strategy.as_str() {
                    crate::router::ROUTE_MANUAL_PRIORITY => {
                        let ma = allowed_m
                            .and_then(|list| {
                                list.iter()
                                    .position(|name| are_models_equivalent(name, &ca.name))
                            })
                            .unwrap_or(usize::MAX);
                        let mb = allowed_m
                            .and_then(|list| {
                                list.iter()
                                    .position(|name| are_models_equivalent(name, &cb.name))
                            })
                            .unwrap_or(usize::MAX);
                        if ma != mb {
                            return ma.cmp(&mb);
                        }
                        let pa = ctx
                            .allowed_channels
                            .as_ref()
                            .and_then(|list| list.iter().position(|name| name == &a.name))
                            .unwrap_or(usize::MAX);
                        let pb = ctx
                            .allowed_channels
                            .as_ref()
                            .and_then(|list| list.iter().position(|name| name == &b.name))
                            .unwrap_or(usize::MAX);
                        pa.cmp(&pb).then(ca.rank.cmp(&cb.rank))
                    }
                    crate::router::ROUTE_LATENCY => ca.lat.cmp(&cb.lat).then(ca.rank.cmp(&cb.rank)),
                    crate::router::ROUTE_ROUND_ROBIN => std::cmp::Ordering::Equal,
                    _ => {
                        // smart_quality
                        ca.rank
                            .cmp(&cb.rank)
                            .then(ca.lat.cmp(&cb.lat))
                            .then(a.name.cmp(&b.name))
                    }
                }
            });
        } else if ctx.strategy == crate::router::ROUTE_MANUAL_PRIORITY {
            if let Some(ref allowed_chans) = ctx.allowed_channels {
                candidates.sort_by_key(|c| {
                    (
                        crate::router::candidate_safety_priority(c),
                        allowed_chans
                            .iter()
                            .position(|name| name == &c.name)
                            .unwrap_or(usize::MAX),
                    )
                });
            }
        }

        let initial_prompt_tokens = estimate_prompt_tokens(&body);
        let is_stream_request = stream_requested
            || inbound
                .get("accept")
                .and_then(|v| v.to_str().ok())
                .map(|v| v.to_lowercase().contains("text/event-stream"))
                .unwrap_or(false);

        let start_overall = Instant::now();
        let mut failed_attempts: Vec<String> = Vec::new();
        let mut trace_steps: Vec<TraceStep> = Vec::new();

        // Create the routing log row immediately, mirroring the Go handler.
        let mut request_log = RequestLog {
            model: effective_model.clone(),
            channel_name: "路由中...".to_string(),
            status_code: 0,
            latency_ms: 0,
            ttft_ms: 0,
            prompt_tokens: initial_prompt_tokens,
            completion_tokens: 0,
            total_tokens: initial_prompt_tokens,
            is_stream: is_stream_request,
            client_ip: ctx.client_ip.clone(),
            user_agent: ctx.user_agent.clone(),
            request_path: ctx.request_path.clone(),
            created_at: chrono::Utc::now(),
            ..Default::default()
        };
        let _ = self.store.record_log(&mut request_log);

        let mut chosen: Option<(
            UpstreamRaw,
            Channel,
            chrono::DateTime<chrono::Utc>,
            KeyLease,
        )> = None;
        let mut last_error: Option<GatewayError> = None;
        let mut claimed = RecoveryClaims {
            router: self.router.clone(),
            ids: Vec::new(),
        };
        let deadline = tokio::time::Instant::now() + Duration::from_secs(600);
        let mut attempts = 0;
        let mut blocked_retry = Duration::ZERO;
        // (channel id, upstream model) pairs actually tried, so cooldown waits
        // are computed for the model the key pool was keyed on, not the
        // client-facing alias or `auto`.
        let mut resolved_models: Vec<(i64, String)> = Vec::new();

        'candidates: for channel in &mut candidates {
            if attempts >= 3 {
                break;
            }
            if channel.circuit_state == CIRCUIT_HALF_OPEN {
                if !self.router.claim_half_open_public(channel.id) {
                    continue;
                }
                claimed.ids.push(channel.id);
            }
            let upstream_models: Vec<String> = if is_auto {
                auto_choices_for_strategy(
                    Some(channel),
                    ctx.allowed_models.as_deref(),
                    &ctx.strategy,
                )
                .into_iter()
                .map(|choice| choice.name)
                .collect()
            } else {
                vec![find_channel_model_for_request(channel, &effective_model)
                    .map(|(actual, _)| actual)
                    .filter(|m| !m.is_empty())
                    .unwrap_or_else(|| effective_model.clone())]
            };
            let mut channel_keys = std::collections::HashSet::new();
            'models: for upstream_model in upstream_models {
                if upstream_model.is_empty() {
                    continue;
                }
                resolved_models.push((channel.id, upstream_model.clone()));
                let mut excluded = std::collections::HashSet::new();
                for _ in 0..2 {
                    if attempts >= 3 {
                        break 'candidates;
                    }
                    // Moving to another model can reuse a credential, but must not
                    // exceed the two-credential budget for this supplier.
                    if channel_keys.len() >= 2 {
                        match self.store.list_keys(channel.id) {
                            Ok(keys) => excluded.extend(
                                keys.iter()
                                    .filter(|key| !channel_keys.contains(&key.id))
                                    .map(|key| key.id),
                            ),
                            Err(_) => {
                                last_error =
                                    Some(GatewayError::Other("credential selection failed".into()));
                                continue 'candidates;
                            }
                        }
                    }
                    let lease = match self.key_pool.acquire_for_channel(
                        channel,
                        &upstream_model,
                        &excluded,
                        None,
                    ) {
                        Ok(Some(lease)) => lease,
                        Ok(None) => {
                            let wait = self.key_pool.retry_after_model(channel.id, &upstream_model);
                            if wait > Duration::ZERO
                                && (blocked_retry == Duration::ZERO || wait < blocked_retry)
                            {
                                blocked_retry = wait;
                            }
                            break;
                        }
                        Err(_) => {
                            last_error =
                                Some(GatewayError::Other("credential selection failed".into()));
                            break;
                        }
                    };
                    excluded.insert(lease.key.id);
                    channel_keys.insert(lease.key.id);
                    // A per-attempt snapshot carries the selected credential. The stored channel is never mutated.
                    let mut selected = channel.clone();
                    selected.api_key = lease.key.secret.clone();
                    let attempt_start = Instant::now();
                    let attempt_started_at = chrono::Utc::now();
                    let mut retry_body = body.clone();
                    let mut adapted = false;
                    loop {
                        attempts += 1;
                        let outcome = tokio::time::timeout_at(
                            deadline,
                            self.prepare_upstream(
                                inbound,
                                &retry_body,
                                &selected,
                                proto,
                                &effective_model,
                                &upstream_model,
                            ),
                        )
                        .await
                        .unwrap_or_else(|_| {
                            Err(GatewayError::Other("total upstream timeout".into()))
                        });
                        let latency = attempt_start.elapsed().as_millis() as i64;
                        let mut step = TraceStep {
                            index: trace_steps.len() as i64 + 1,
                            channel_id: channel.id,
                            channel_name: channel.name.clone(),
                            key_id: lease.key.id,
                            key_name: lease.key.name.clone(),
                            endpoint: crate::probe::redact_endpoint(&channel.endpoint),
                            model: upstream_model.clone(),
                            attempt: attempts,
                            latency_ms: latency,
                            ..Default::default()
                        };
                        match outcome {
                            Ok(upstream) => {
                                step.status_code = upstream.status as i64;
                                step.ttft_ms = upstream
                                    .ttft
                                    .as_ref()
                                    .and_then(|m| m.lock().ok())
                                    .and_then(|m| m.tracker.ttft_ms())
                                    .unwrap_or(0);
                                step.succeeded = true;
                                trace_steps.push(step);
                                chosen = Some((upstream, selected, attempt_started_at, lease));
                                break 'candidates;
                            }
                            Err(mut err) => {
                                err.redact(&lease.key.secret);
                                let info = err.failure_info();
                                let message = err.to_string();
                                step.status_code = info.status_code as i64;
                                step.error = message.clone();
                                trace_steps.push(step);
                                if !adapted
                                    && attempts < 3
                                    && info.class != Some(FailureClass::RateLimit)
                                {
                                    if let Some(updated) =
                                        adapt_unsupported_reasoning_effort(&body, &message)
                                    {
                                        adapted = true;
                                        retry_body = updated;
                                        continue;
                                    }
                                }
                                failed_attempts.push(format!(
                                    "{} / {} ({})",
                                    channel.name, lease.key.name, info.status_code
                                ));
                                if info.class == Some(FailureClass::Client) {
                                    last_error = Some(err);
                                    break 'candidates;
                                }
                                if self
                                    .store
                                    .channel_config_is_current(channel, &upstream_model)
                                    .unwrap_or(false)
                                {
                                    if lease.is_current(&upstream_model).unwrap_or(false) {
                                        self.router.apply_live_outcome(
                                            channel,
                                            &upstream_model,
                                            Some(&info),
                                            &message,
                                            latency,
                                            attempt_started_at,
                                        );
                                    }
                                    let _ = lease.failure(&info, &upstream_model);
                                }
                                let key_fault = matches!(
                                    info.class,
                                    Some(
                                        FailureClass::RateLimit
                                            | FailureClass::Auth
                                            | FailureClass::Model
                                    )
                                );
                                let channel_has_other_models = !channel.extra_models.is_empty();
                                last_error = Some(err);
                                if !key_fault {
                                    if is_auto && channel_has_other_models {
                                        continue 'models;
                                    }
                                    continue 'candidates;
                                }
                                break;
                            }
                        }
                    }
                }
            }
        }
        if chosen.is_none() {
            // Include leases which just entered cooldown, not only channels initially exhausted.
            for (channel_id, model) in &resolved_models {
                let wait = self.key_pool.retry_after_model(*channel_id, model);
                if wait > Duration::ZERO
                    && (blocked_retry == Duration::ZERO || wait < blocked_retry)
                {
                    blocked_retry = wait;
                }
            }
            if blocked_retry > Duration::ZERO
                && (last_error.is_none()
                    || last_error
                        .as_ref()
                        .is_some_and(|e| e.failure_info().status_code == 429))
            {
                last_error = Some(GatewayError::UpstreamStatus {
                    status: 429,
                    retry_after: blocked_retry,
                    message: "可用 Key 正在冷却".into(),
                });
            } else if last_error.is_none() {
                last_error = Some(GatewayError::upstream_status(503, "没有可用 Key".into()));
            }
        }

        let total_latency = start_overall.elapsed().as_millis() as i64;

        let Some((upstream, winning, winning_started_at, key_lease)) = chosen else {
            return self.finish_all_failed(
                proto,
                &effective_model,
                &failed_attempts,
                last_error,
                request_log,
                total_latency,
                &trace_steps,
            );
        };

        self.deliver(
            proto,
            upstream,
            winning,
            winning_started_at,
            &failed_attempts,
            &trace_steps,
            request_log,
            start_overall,
            ctx,
            total_latency,
            key_lease,
            deadline,
            claimed,
        )
        .await
    }

    /// Buffers enough of the response to validate it before committing to the client.
    #[allow(clippy::too_many_arguments)]
    async fn prepare_upstream(
        &self,
        inbound: &HeaderMap,
        body: &[u8],
        channel: &Channel,
        proto: GatewayProtocol,
        client_model: &str,
        upstream_model: &str,
    ) -> Result<UpstreamRaw, GatewayError> {
        let started = Instant::now();
        let started_at = chrono::Utc::now();
        let mut upstream = self
            .forward_upstream(inbound, body, channel, proto, client_model, upstream_model)
            .await?;
        upstream.expected_choices = if proto == GatewayProtocol::Chat {
            serde_json::from_slice::<serde_json::Value>(body)
                .ok()
                .and_then(|v| v.get("n").and_then(|n| n.as_u64()))
                .and_then(|n| usize::try_from(n).ok())
                .unwrap_or(1)
                .max(1)
        } else {
            1
        };
        let is_sse = header_contains(&upstream.headers, "content-type", "text/event-stream");
        if is_sse && (200..300).contains(&upstream.status) && upstream.ttft.is_none() {
            let measurement = Arc::new(Mutex::new(TtftMeasurement {
                started,
                started_at,
                tracker: StreamTtftTracker::default(),
            }));
            let observer = measurement.clone();
            // Observe before prefix buffering/replay and client backpressure.
            upstream.stream = Box::pin(upstream.stream.map(move |item| {
                if let Ok(chunk) = &item {
                    if let Ok(mut measurement) = observer.lock() {
                        measurement
                            .tracker
                            .feed(chunk, started.elapsed().as_millis() as i64);
                    }
                }
                item
            }));
            upstream.ttft = Some(measurement);
        }
        let (prefix, rest, read_err) = read_initial_prefix_with_choices(
            upstream.stream,
            is_sse && (200..300).contains(&upstream.status),
            upstream.expected_choices,
        )
        .await;
        if !(200..300).contains(&upstream.status) {
            return Err(GatewayError::UpstreamStatus {
                status: upstream.status as i32,
                retry_after: retry_after_from(&upstream.headers),
                message: parse_upstream_error_message(&String::from_utf8_lossy(&prefix)),
            });
        }
        // No bytes have reached the client yet, so body errors can fail over.
        if let Some(err) = read_err {
            return Err(GatewayError::StreamStart {
                message: err.to_string(),
            });
        }
        if prefix.is_empty() {
            return Err(GatewayError::StreamStart {
                message: "empty response".into(),
            });
        }
        let text = String::from_utf8_lossy(&prefix);
        if is_sse {
            if let Some((message, status)) = upstream_stream_error(&text) {
                return Err(GatewayError::UpstreamStatus {
                    status,
                    retry_after: retry_after_from(&upstream.headers),
                    message,
                });
            }
        } else {
            let value: serde_json::Value =
                serde_json::from_slice(&prefix).map_err(|err| GatewayError::StreamStart {
                    message: format!("invalid upstream JSON response: {err}"),
                })?;
            if !value.is_object() {
                return Err(GatewayError::StreamStart {
                    message: "invalid upstream JSON response: expected an object".into(),
                });
            }
            if let Some((message, status)) = upstream_error_payload(&text) {
                return Err(GatewayError::UpstreamStatus {
                    status,
                    retry_after: retry_after_from(&upstream.headers),
                    message,
                });
            }
            validate_inference_response(proto, &value)?;
        }
        upstream.stream =
            Box::pin(stream::once(async move { Ok(Bytes::from(prefix)) }).chain(rest));
        Ok(upstream)
    }

    fn resolve_model_alias(&self, requested: &str) -> String {
        let requested = requested.trim();
        if requested.is_empty() {
            return String::new();
        }
        let Ok(channels) = self.store.list() else {
            return requested.to_string();
        };
        for ch in &channels {
            if ch.enabled && channel_supports_model(ch, requested) {
                return requested.to_string();
            }
        }
        let lower = requested.to_lowercase();
        if lower.starts_with("claude-") || lower.starts_with("anthropic/claude-") {
            let mut benched: Option<String> = None;
            for ch in &channels {
                let model = ch.primary_model.trim().to_string();
                if !ch.enabled || model.is_empty() {
                    continue;
                }
                if channel_serving_traffic(ch) {
                    return model;
                }
                if benched.is_none() {
                    benched = Some(model);
                }
            }
            if let Some(model) = benched {
                return model;
            }
        }
        requested.to_string()
    }

    /// Sends the request upstream, translating when the client speaks Anthropic
    /// but the channel is OpenAI-compatible.
    async fn forward_upstream(
        &self,
        inbound: &HeaderMap,
        body: &[u8],
        channel: &Channel,
        proto: GatewayProtocol,
        client_model: &str,
        upstream_model: &str,
    ) -> Result<UpstreamRaw, GatewayError> {
        if proto == GatewayProtocol::Anthropic && channel.provider == PROVIDER_OPENAI {
            return self
                .forward_translated(inbound, body, channel, client_model, upstream_model)
                .await;
        }

        let path = match proto {
            GatewayProtocol::Anthropic => "/v1/messages",
            GatewayProtocol::Responses => "/v1/responses",
            GatewayProtocol::Chat => "/v1/chat/completions",
        };
        let target = format!("{}{path}", normalize_endpoint(&channel.endpoint));

        // Rewrite the model and, for non-Anthropic streams, request usage.
        let send_body = match serde_json::from_slice::<serde_json::Value>(body) {
            Ok(serde_json::Value::Object(mut payload)) => {
                let mut modified = false;
                if !upstream_model.is_empty() {
                    payload.insert(
                        "model".to_string(),
                        serde_json::Value::String(upstream_model.to_string()),
                    );
                    modified = true;
                }
                let streaming = payload
                    .get("stream")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if streaming
                    && proto == GatewayProtocol::Chat
                    && !payload.contains_key("stream_options")
                {
                    payload.insert("stream_options".to_string(), json!({"include_usage": true}));
                    modified = true;
                }
                if modified {
                    serde_json::to_vec(&serde_json::Value::Object(payload))
                        .unwrap_or_else(|_| body.to_vec())
                } else {
                    body.to_vec()
                }
            }
            _ => body.to_vec(),
        };

        let mut request = self.client.post(&target).body(send_body);
        for (name, value) in inbound.iter() {
            if crate::api::forward::is_forwardable_client_header(name.as_str()) {
                request = request.header(name, value);
            }
        }
        request = request.header("content-type", "application/json");
        if proto == GatewayProtocol::Anthropic {
            if !channel.api_key.is_empty() {
                request = request.header("x-api-key", &channel.api_key);
            }
            if inbound.get("anthropic-version").is_none() {
                request = request.header("anthropic-version", "2023-06-01");
            }
        } else if !channel.api_key.is_empty() {
            request = request.header("authorization", format!("Bearer {}", channel.api_key));
        }

        let response = request
            .send()
            .await
            .map_err(|err| GatewayError::Transport {
                message: err.to_string(),
                client_closed: false,
            })?;

        Ok(raw_from_response(response))
    }

    /// Sends an Anthropic request to an OpenAI Chat upstream and converts the
    /// reply back to the Anthropic wire format.
    async fn forward_translated(
        &self,
        inbound: &HeaderMap,
        body: &[u8],
        channel: &Channel,
        client_model: &str,
        upstream_model: &str,
    ) -> Result<UpstreamRaw, GatewayError> {
        let started = Instant::now();
        let started_at = chrono::Utc::now();
        let mut msg_req: MessagesRequest = serde_json::from_slice(body)
            .map_err(|err| GatewayError::Client(format!("parse anthropic request: {err}")))?;
        if !client_model.is_empty() {
            msg_req.model = client_model.to_string();
        }
        let mut chat_req = messages_to_chat(Some(&msg_req))
            .map_err(|err| GatewayError::Client(format!("convert anthropic request: {err}")))?;
        chat_req.model = upstream_model.to_string();
        let chat_body = serde_json::to_vec(&chat_req)
            .map_err(|err| GatewayError::Other(format!("encode chat request: {err}")))?;

        let target = format!(
            "{}/v1/chat/completions",
            normalize_endpoint(&channel.endpoint)
        );
        let mut request = self.client.post(&target).body(chat_body);
        for (name, value) in inbound.iter() {
            if crate::api::forward::is_forwardable_client_header(name.as_str())
                && name.as_str() != "anthropic-version"
                && name.as_str() != "anthropic-beta"
            {
                request = request.header(name, value);
            }
        }
        request = request.header("content-type", "application/json");
        request = request.header("accept", "application/json, text/event-stream");
        if !channel.api_key.is_empty() {
            request = request.header("authorization", format!("Bearer {}", channel.api_key));
        }

        let response = request
            .send()
            .await
            .map_err(|err| GatewayError::Transport {
                message: err.to_string(),
                client_closed: false,
            })?;
        let status = response.status().as_u16();

        if status >= 400 {
            let retry_after = retry_after_from(response.headers());
            let body_bytes =
                crate::probe::read_response_limited(response, crate::probe::MAX_ERROR_BODY_READ)
                    .await
                    .unwrap_or_default();
            let body_text = String::from_utf8_lossy(&body_bytes);
            let mut message = parse_upstream_error_message(&body_text);
            if message.is_empty() {
                message = format!("HTTP {status}");
            }
            let payload = json!({
                "type": "error",
                "error": {
                    "type": anthropic_error_type(status as i32),
                    "message": format!("Upstream returned HTTP {status}: {message}"),
                }
            });
            let mut headers = HeaderMap::new();
            headers.insert("content-type", HeaderValue::from_static("application/json"));
            if retry_after != Duration::ZERO {
                if let Ok(value) = HeaderValue::from_str(&retry_after.as_secs().to_string()) {
                    headers.insert("retry-after", value);
                }
            }
            return Ok(UpstreamRaw {
                status,
                headers,
                stream: single_chunk(serde_json::to_vec(&payload).unwrap_or_default()),
                ttft: None,
                expected_choices: 1,
                non_stream_usage_authoritative: false,
            });
        }

        if !msg_req.stream {
            let resp_bytes = crate::probe::read_response_limited(response, MAX_REQUEST_BODY)
                .await
                .map_err(|err| GatewayError::StreamStart { message: err })?;
            if let Some((message, err_status)) =
                upstream_error_payload(&String::from_utf8_lossy(&resp_bytes))
            {
                return Err(GatewayError::upstream_status(err_status, message));
            }
            let value =
                serde_json::from_slice(&resp_bytes).map_err(|err| GatewayError::StreamStart {
                    message: format!("invalid chat response: {err}"),
                })?;
            validate_inference_response(GatewayProtocol::Chat, &value)?;
            let chat_resp: crate::protocol::ChatResponse = serde_json::from_slice(&resp_bytes)
                .map_err(|err| GatewayError::Other(format!("decode chat response: {err}")))?;
            let messages = chat_to_messages(&chat_resp, &msg_req.model);
            let non_stream_usage_authoritative =
                crate::api::forward::exact_non_streaming_tokens(&resp_bytes).is_some();
            let out = serde_json::to_vec(&messages)
                .map_err(|err| GatewayError::Other(format!("encode anthropic response: {err}")))?;
            let mut headers = HeaderMap::new();
            headers.insert("content-type", HeaderValue::from_static("application/json"));
            return Ok(UpstreamRaw {
                status: 200,
                headers,
                stream: single_chunk(out),
                ttft: None,
                expected_choices: 1,
                non_stream_usage_authoritative,
            });
        }

        // Streaming: translate each OpenAI chunk into Anthropic events.
        let model = msg_req.model.clone();
        let ttft = Arc::new(Mutex::new(TtftMeasurement {
            started,
            started_at,
            tracker: StreamTtftTracker::default(),
        }));
        let observer = ttft.clone();
        let upstream_stream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>> =
            Box::pin(response.bytes_stream().map(move |item| {
                if let Ok(chunk) = &item {
                    if let Ok(mut measurement) = observer.lock() {
                        measurement
                            .tracker
                            .feed(chunk, started.elapsed().as_millis() as i64);
                    }
                }
                item
            }));
        let out_stream = translated_stream(upstream_stream, model);
        let mut headers = HeaderMap::new();
        headers.insert(
            "content-type",
            HeaderValue::from_static("text/event-stream"),
        );
        headers.insert("cache-control", HeaderValue::from_static("no-cache"));
        headers.insert("connection", HeaderValue::from_static("keep-alive"));
        Ok(UpstreamRaw {
            status: 200,
            headers,
            stream: Box::pin(out_stream),
            ttft: Some(ttft),
            expected_choices: 1,
            non_stream_usage_authoritative: true,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn deliver(
        &self,
        proto: GatewayProtocol,
        upstream: UpstreamRaw,
        winning: Channel,
        winning_started_at: chrono::DateTime<chrono::Utc>,
        failed_attempts: &[String],
        trace_steps: &[TraceStep],
        mut request_log: RequestLog,
        start: Instant,
        ctx: &RequestContext,
        total_latency: i64,
        key_lease: KeyLease,
        deadline: tokio::time::Instant,
        recovery_claims: RecoveryClaims,
    ) -> Response {
        let is_stream = header_contains(&upstream.headers, "content-type", "text/event-stream");
        let failover_from = failed_attempts.join(" → ");

        if request_log.id > 0 {
            request_log.channel_name = winning.name.clone();
            request_log.upstream_model = trace_steps
                .last()
                .map(|step| step.model.clone())
                .unwrap_or_default();
            request_log.endpoint = crate::probe::redact_endpoint(&winning.endpoint);
            request_log.is_stream = is_stream;
            request_log.is_failover = !failed_attempts.is_empty();
            request_log.failover_from = failover_from.clone();
            request_log.ttft_ms = 0;
            request_log.trace_json = serde_json::to_string(trace_steps).unwrap_or_default();
            let _ = self.store.update_log(&request_log);
        }

        let mut tracker = StreamTokenTracker::new(request_log.prompt_tokens);
        tracker.expected_choices = upstream.expected_choices;
        tracker.translated_from_chat =
            proto == GatewayProtocol::Anthropic && winning.provider == PROVIDER_OPENAI;
        let tracker = Arc::new(Mutex::new(tracker));
        let log = Arc::new(Mutex::new(request_log));
        let mut logging = LoggingStream::new(
            upstream.stream,
            tracker,
            log,
            self.store.clone(),
            Some(winning.clone()),
            start,
            is_stream,
        );
        logging.key_lease = Some(key_lease);
        logging.recovery_claims = Some(recovery_claims);
        logging.router = Some(self.router.clone());
        logging.outcome_started_at = winning_started_at;
        logging.ttft = upstream.ttft;
        logging.non_stream_usage_authoritative = upstream.non_stream_usage_authoritative;
        logging.deadline = Some(Box::pin(tokio::time::sleep_until(deadline)));
        let body = Body::from_stream(logging);

        let mut response = Response::new(body);
        *response.status_mut() = StatusCode::from_u16(upstream.status).unwrap_or(StatusCode::OK);
        for (name, value) in upstream.headers.iter() {
            if is_hop_by_hop_header(name.as_str()) {
                continue;
            }
            response.headers_mut().append(name, value.clone());
        }
        let headers = response.headers_mut();
        headers.insert(
            "x-heigate-channel",
            HeaderValue::from_str(&winning.name).unwrap_or_else(|_| HeaderValue::from_static("")),
        );
        headers.insert(
            "x-heigate-strategy",
            HeaderValue::from_str(&ctx.strategy).unwrap_or_else(|_| HeaderValue::from_static("")),
        );
        headers.insert(
            "x-heigate-latency-ms",
            HeaderValue::from_str(&total_latency.to_string())
                .unwrap_or_else(|_| HeaderValue::from_static("0")),
        );
        if !failover_from.is_empty() {
            if let Ok(value) = HeaderValue::from_str(&failover_from) {
                headers.insert("x-heigate-failover", value);
            }
        }
        response
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_all_failed(
        &self,
        proto: GatewayProtocol,
        model: &str,
        failed_attempts: &[String],
        last_error: Option<GatewayError>,
        mut request_log: RequestLog,
        total_latency: i64,
        trace_steps: &[TraceStep],
    ) -> Response {
        let failure_info = last_error
            .as_ref()
            .map(FailureInfoProvider::failure_info)
            .unwrap_or_default();
        let mut status = StatusCode::BAD_GATEWAY;
        if failure_info.status_code == 429 {
            status = StatusCode::TOO_MANY_REQUESTS;
        } else if (400..500).contains(&failure_info.status_code) || failure_info.status_code == 503
        {
            status = StatusCode::from_u16(failure_info.status_code as u16)
                .unwrap_or(StatusCode::BAD_GATEWAY);
        }

        let failover_desc = failed_attempts.join(" → ");
        let error_message = last_error
            .as_ref()
            .map(|err| err.to_string())
            .unwrap_or_else(|| "all configured channels failed".to_string());
        let trace_json = serde_json::to_string(trace_steps).unwrap_or_default();

        request_log.channel_name = "全渠道失败".to_string();
        request_log.status_code = status.as_u16() as i64;
        request_log.finish_reason = "error".into();
        request_log.latency_ms = total_latency;
        request_log.ttft_ms = 0;
        request_log.error_message = error_message.clone();
        request_log.failover_from = failover_desc.clone();
        request_log.is_failover = failed_attempts.len() > 1;
        request_log.trace_json = trace_json;
        if request_log.id > 0 {
            let _ = self.store.update_log(&request_log);
        } else {
            let _ = self.store.record_log(&mut request_log);
        }

        let response = if proto == GatewayProtocol::Anthropic {
            anthropic_error(
                status,
                anthropic_error_type(status.as_u16() as i32),
                &format!(
                    "All configured channels failed for model {model:?} ({}): {error_message}",
                    failed_attempts.join(" -> ")
                ),
            )
        } else {
            let body = json!({
                "error": "all configured channels failed",
                "failure_class": crate::resilience::failure_label(&failure_info),
            })
            .to_string();
            let mut response = Response::new(Body::from(body));
            *response.status_mut() = status;
            response
                .headers_mut()
                .insert("content-type", HeaderValue::from_static("application/json"));
            response
        };
        let mut response = response;
        if failure_info.retry_after > Duration::ZERO {
            if let Ok(value) =
                HeaderValue::from_str(&failure_info.retry_after.as_secs().to_string())
            {
                response.headers_mut().insert("retry-after", value);
            }
        }
        response
    }
}

/// Reads a `Retry-After` header into a delay, defaulting to zero when it is
/// absent or unparseable. Shared by the forward loop and the translated path.
fn retry_after_from(headers: &HeaderMap) -> Duration {
    headers
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .map(parse_retry_after)
        .unwrap_or(Duration::ZERO)
}

fn header_contains(headers: &HeaderMap, name: &str, needle: &str) -> bool {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_lowercase().contains(needle))
        .unwrap_or(false)
}

fn raw_from_response(response: reqwest::Response) -> UpstreamRaw {
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let stream: ByteStream = Box::pin(
        response
            .bytes_stream()
            .map(|item| item.map_err(std::io::Error::other)),
    );
    UpstreamRaw {
        status,
        headers,
        stream,
        ttft: None,
        expected_choices: 1,
        non_stream_usage_authoritative: true,
    }
}

fn single_chunk(data: Vec<u8>) -> ByteStream {
    Box::pin(stream::once(async move { Ok(Bytes::from(data)) }))
}

fn validate_inference_response(
    proto: GatewayProtocol,
    value: &serde_json::Value,
) -> Result<(), GatewayError> {
    use serde_json::Value;
    let valid = match proto {
        GatewayProtocol::Chat => {
            value
                .get("choices")
                .and_then(Value::as_array)
                .is_some_and(|choices| {
                    !choices.is_empty()
                        && choices.iter().all(|choice| {
                            choice
                                .get("message")
                                .filter(|m| m.is_object())
                                .is_some_and(|message| {
                                    message.get("content").is_some_and(|c| {
                                        c.is_string() || c.is_array() || c.is_null()
                                    }) || message
                                        .get("tool_calls")
                                        .and_then(Value::as_array)
                                        .is_some_and(|calls| !calls.is_empty())
                                        || message
                                            .get("function_call")
                                            .is_some_and(Value::is_object)
                                        || message.get("refusal").is_some_and(Value::is_string)
                                        || message
                                            .get("reasoning_content")
                                            .is_some_and(Value::is_string)
                                        || message.get("reasoning").is_some_and(Value::is_string)
                                        || message.get("audio").is_some_and(Value::is_object)
                                        || matches!(
                                            choice.get("finish_reason").and_then(Value::as_str),
                                            Some("length" | "content_filter")
                                        )
                                })
                        })
                })
        }
        GatewayProtocol::Anthropic => {
            value.get("type").and_then(Value::as_str) == Some("message")
                && value
                    .get("content")
                    .and_then(Value::as_array)
                    .is_some_and(|blocks| blocks.iter().all(Value::is_object))
        }
        GatewayProtocol::Responses => {
            matches!(
                value.get("status").and_then(Value::as_str),
                Some("completed" | "incomplete")
            ) && value.get("output").is_some_and(|output| {
                output
                    .as_array()
                    .is_some_and(|items| items.iter().all(Value::is_object))
                    || output.is_object()
            })
        }
    };
    if valid {
        Ok(())
    } else {
        Err(GatewayError::StreamStart {
            message: format!("invalid {proto:?} inference response structure"),
        })
    }
}

/// Buffers non-SSE bodies, or the first SSE output, so failures can fail over
/// before any byte reaches the client.
#[cfg(test)]
async fn read_initial_prefix(
    stream: ByteStream,
    is_sse: bool,
) -> (Vec<u8>, ByteStream, Option<std::io::Error>) {
    read_initial_prefix_with_choices(stream, is_sse, 1).await
}

async fn read_initial_prefix_with_choices(
    mut stream: ByteStream,
    is_sse: bool,
    expected_choices: usize,
) -> (Vec<u8>, ByteStream, Option<std::io::Error>) {
    stream = Box::pin(stream.fuse());
    let mut prefix: Vec<u8> = Vec::new();
    if !is_sse {
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(chunk) => {
                    if chunk.len() > MAX_REQUEST_BODY - prefix.len() {
                        return (
                            prefix,
                            stream,
                            Some(std::io::Error::other("upstream response body too large")),
                        );
                    }
                    prefix.extend_from_slice(&chunk);
                }
                Err(err) => return (prefix, stream, Some(err)),
            }
        }
        (prefix, stream, None)
    } else {
        let mut tracker = StreamTokenTracker::default();
        tracker.expected_choices = expected_choices;
        let mut has_output = false;
        loop {
            match stream.next().await {
                Some(Ok(chunk)) => {
                    let mut consumed = 0;
                    while consumed < chunk.len() {
                        let remaining = &chunk[consumed..];
                        let part_len = crate::protocol::stream::next_sse_line_end(remaining)
                            .unwrap_or(remaining.len());
                        let part = &remaining[..part_len];
                        if part.len()
                            > (MAX_STREAM_PREFIX_BYTES
                                + crate::protocol::stream::MAX_SSE_LINE_BYTES)
                                .saturating_sub(prefix.len())
                        {
                            return (
                                prefix,
                                stream,
                                Some(std::io::Error::other(
                                    "upstream stream prefix limit exceeded",
                                )),
                            );
                        }
                        tracker.feed(part);
                        if tracker.buffer_exceeded {
                            return (
                                prefix,
                                stream,
                                Some(std::io::Error::other(tracker.stream_error)),
                            );
                        }
                        prefix.extend_from_slice(part);
                        consumed += part.len();
                        has_output |= tracker.saw_output;
                        if sse_event_boundary(&prefix)
                            && (has_output || !tracker.stream_error.is_empty() || tracker.saw_done)
                        {
                            // Replay the unused bytes without treating a transport chunk as an event.
                            if consumed < chunk.len() {
                                let tail = chunk.slice(consumed..);
                                stream =
                                    Box::pin(stream::once(async move { Ok(tail) }).chain(stream));
                            }
                            return (prefix, stream, None);
                        }
                        if !part
                            .last()
                            .is_some_and(|byte| matches!(byte, b'\r' | b'\n'))
                        {
                            continue;
                        }
                        // Metadata is capped at 64 KB; one output line may use the SSE line limit.
                        if !has_output && prefix.len() >= MAX_STREAM_PREFIX_BYTES {
                            return (
                                prefix,
                                stream,
                                Some(std::io::Error::other(
                                    "stream prefix limit exceeded before model output",
                                )),
                            );
                        }
                    }
                }
                Some(Err(err)) => return (prefix, stream, Some(err)),
                None => {
                    tracker.finish();
                    let text = String::from_utf8_lossy(&prefix);
                    if tracker.saw_done || upstream_stream_error(&text).is_some() {
                        return (prefix, stream, None);
                    }
                    return (
                        prefix,
                        stream,
                        Some(std::io::Error::other(
                            "truncated stream before model output",
                        )),
                    );
                }
            }
        }
    }
}

fn sse_event_boundary(bytes: &[u8]) -> bool {
    bytes.ends_with(b"\n\n") || bytes.ends_with(b"\r\r") || bytes.ends_with(b"\r\n\r\n")
}

/// Converts an OpenAI SSE byte stream into an Anthropic SSE byte stream.
fn translated_stream(
    upstream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    model: String,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send {
    struct State {
        upstream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
        decoder: crate::protocol::stream::SseEventDecoder,
        state: StreamState,
        finished: bool,
        pending: std::collections::VecDeque<Result<Bytes, std::io::Error>>,
        saw_done: bool,
        saw_finish: bool,
    }

    stream::unfold(
        State {
            upstream,
            decoder: crate::protocol::stream::SseEventDecoder::default(),
            state: StreamState::new(&model),
            finished: false,
            pending: std::collections::VecDeque::new(),
            saw_done: false,
            saw_finish: false,
        },
        |mut st| async move {
            loop {
                if let Some(item) = st.pending.pop_front() {
                    return Some((item, st));
                }
                if st.finished {
                    return None;
                }
                match st.upstream.next().await {
                    Some(Ok(chunk)) => {
                        let events = st.decoder.feed(&chunk);
                        let decoder_exceeded = st.decoder.exceeded;
                        for payload in events
                            .into_iter()
                            .map(|data| String::from_utf8_lossy(&data).into_owned())
                        {
                            let done = payload == "[DONE]";
                            if done {
                                st.saw_done = true;
                                break;
                            }
                            if let Some((message, status)) = upstream_error_payload(&payload) {
                                st.finished = true;
                                let event = json!({
                                    "type": "error",
                                    "error": {
                                        "type": anthropic_error_type(status),
                                        "code": status,
                                        "message": message,
                                    },
                                });
                                if let Ok(frame) = encode_sse(&event) {
                                    st.pending.push_back(Ok(Bytes::from(frame)));
                                }
                                st.pending
                                    .push_back(Err(std::io::Error::other("upstream stream error")));
                                break;
                            }
                            let Ok(chunk) = serde_json::from_str::<ChatChunk>(&payload) else {
                                continue;
                            };
                            for choice in &chunk.choices {
                                if choice
                                    .finish_reason
                                    .as_deref()
                                    .map(|f| !f.trim().is_empty())
                                    .unwrap_or(false)
                                {
                                    st.saw_finish = true;
                                }
                            }
                            let events = st.state.feed(Some(&chunk));
                            if st.state.buffer_limit_exceeded() {
                                st.finished = true;
                                st.pending.push_back(Err(std::io::Error::other(
                                    "upstream tool arguments exceeded size limit",
                                )));
                                break;
                            }
                            for event in events {
                                if let Ok(frame) = encode_sse(&event) {
                                    st.pending.push_back(Ok(Bytes::from(frame)));
                                }
                            }
                        }
                        // Preserve complete events decoded before the oversized
                        // line, then terminate with the size error. Do not turn
                        // a [DONE] found earlier in the same network chunk into
                        // a successful Anthropic message after a decoder error.
                        if !st.finished && decoder_exceeded {
                            st.finished = true;
                            st.pending.push_back(Err(std::io::Error::other(
                                "upstream SSE line or event exceeded size limit",
                            )));
                        } else if !st.finished && st.saw_done {
                            st.finished = true;
                            for event in st.state.finish() {
                                if let Ok(frame) = encode_sse(&event) {
                                    st.pending.push_back(Ok(Bytes::from(frame)));
                                }
                            }
                        }
                    }
                    Some(Err(err)) => {
                        st.finished = true;
                        st.pending.push_back(Err(std::io::Error::other(err)));
                    }
                    None => {
                        st.finished = true;
                        let pending_events = st.decoder.finish();
                        if st.decoder.exceeded {
                            st.pending.push_back(Err(std::io::Error::other(
                                "upstream SSE line or event exceeded size limit",
                            )));
                            continue;
                        }
                        let mut final_error = false;
                        for payload in pending_events
                            .into_iter()
                            .map(|data| String::from_utf8_lossy(&data).into_owned())
                        {
                            if let Some((message, status)) = upstream_error_payload(&payload) {
                                let event = json!({
                                    "type": "error",
                                    "error": {
                                        "type": anthropic_error_type(status),
                                        "code": status,
                                        "message": message,
                                    },
                                });
                                if let Ok(frame) = encode_sse(&event) {
                                    st.pending.push_back(Ok(Bytes::from(frame)));
                                }
                                final_error = true;
                                continue;
                            }
                            let Ok(chunk) = serde_json::from_str::<ChatChunk>(&payload) else {
                                continue;
                            };
                            for choice in &chunk.choices {
                                st.saw_finish |= choice
                                    .finish_reason
                                    .as_deref()
                                    .is_some_and(|f| !f.trim().is_empty());
                            }
                            for event in st.state.feed(Some(&chunk)) {
                                if let Ok(frame) = encode_sse(&event) {
                                    st.pending.push_back(Ok(Bytes::from(frame)));
                                }
                            }
                        }
                        if final_error {
                            st.finished = true;
                            st.pending
                                .push_back(Err(std::io::Error::other("upstream stream error")));
                            continue;
                        }
                        if !st.saw_done && !st.saw_finish {
                            st.pending.push_back(Err(std::io::Error::new(
                                std::io::ErrorKind::UnexpectedEof,
                                "stream truncated",
                            )));
                        } else {
                            for event in st.state.finish() {
                                if let Ok(frame) = encode_sse(&event) {
                                    st.pending.push_back(Ok(Bytes::from(frame)));
                                }
                            }
                        }
                    }
                }
            }
        },
    )
}

fn json_error(status: StatusCode, message: &str) -> Response {
    let body = json!({ "error": message }).to_string();
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert("content-type", HeaderValue::from_static("application/json"));
    response
}

fn anthropic_error(status: StatusCode, kind: &str, message: &str) -> Response {
    let body = json!({
        "type": "error",
        "error": { "type": kind, "message": message }
    })
    .to_string();
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert("content-type", HeaderValue::from_static("application/json"));
    response
}

/// A stream that updates the request log and token tracker as it flows, then
/// finalizes the log when the upstream ends.
pub struct LoggingStream {
    ttft: Option<Arc<Mutex<TtftMeasurement>>>,
    deadline: Option<Pin<Box<tokio::time::Sleep>>>,
    key_lease: Option<KeyLease>,
    recovery_claims: Option<RecoveryClaims>,
    router: Option<Arc<Router>>,
    outcome_started_at: chrono::DateTime<chrono::Utc>,
    client_cancelled: bool,
    non_stream_body: Vec<u8>,
    non_stream_usage_authoritative: bool,
    inner: ByteStream,
    tracker: Arc<Mutex<StreamTokenTracker>>,
    log: Arc<Mutex<RequestLog>>,
    store: Arc<Store>,
    winning: Option<Channel>,
    start: Instant,
    is_stream: bool,
    finished: bool,
    last_update: Instant,
    last_reported: i64,
    saw_error: bool,
    error_message: String,
}

impl LoggingStream {
    pub fn new(
        inner: ByteStream,
        tracker: Arc<Mutex<StreamTokenTracker>>,
        log: Arc<Mutex<RequestLog>>,
        store: Arc<Store>,
        winning: Option<Channel>,
        start: Instant,
        is_stream: bool,
    ) -> Self {
        LoggingStream {
            ttft: None,
            deadline: None,
            key_lease: None,
            recovery_claims: None,
            router: None,
            outcome_started_at: chrono::Utc::now(),
            client_cancelled: false,
            non_stream_body: Vec::new(),
            non_stream_usage_authoritative: true,
            inner,
            tracker,
            log,
            store,
            winning,
            start,
            is_stream,
            finished: false,
            last_update: Instant::now(),
            last_reported: 0,
            saw_error: false,
            error_message: String::new(),
        }
    }

    fn finalize(&mut self) {
        if let Ok(mut tracker) = self.tracker.try_lock() {
            tracker.finish();
        }
        let snapshot = match self.tracker.try_lock() {
            Ok(tracker) => (
                tracker.prompt_tokens,
                tracker.completion_tokens,
                tracker.total_tokens,
                tracker.saw_done,
                tracker.finish_reason.clone(),
                tracker.stream_error.clone(),
                tracker.stream_error_status,
                tracker.has_exact_prompt_tokens,
            ),
            Err(_) => return,
        };
        let Ok(mut log) = self.log.try_lock() else {
            return;
        };
        let (
            prompt_tokens,
            completion_tokens,
            total_tokens,
            saw_done,
            finish_reason,
            stream_error,
            stream_error_status,
            has_exact_prompt_tokens,
        ) = snapshot;
        let upstream_failed = !self.client_cancelled || !stream_error.is_empty();
        if !stream_error.is_empty() {
            self.saw_error = true;
            self.error_message = stream_error;
        }
        if let Some(lease) = &self.key_lease {
            if !lease.key.secret.is_empty() {
                self.error_message = self.error_message.replace(&lease.key.secret, "[REDACTED]");
            }
        }
        log.latency_ms = self.start.elapsed().as_millis() as i64;
        if self.is_stream {
            if prompt_tokens > 0 || has_exact_prompt_tokens {
                log.prompt_tokens = prompt_tokens;
            }
            log.completion_tokens = completion_tokens;
            log.total_tokens = if total_tokens > 0 {
                total_tokens
            } else {
                log.prompt_tokens + log.completion_tokens
            };
            if self.saw_error {
                log.status_code = 502;
                log.finish_reason = "error".to_string();
                log.error_message = self.error_message.clone();
            } else if saw_done {
                log.status_code = 200;
                log.error_message.clear();
                log.finish_reason = if finish_reason.is_empty() {
                    "stop".to_string()
                } else {
                    finish_reason
                };
            } else {
                log.status_code = 502;
                log.finish_reason = "error".to_string();
                log.error_message = "流式传输中断: truncated stream".to_string();
            }
        } else {
            log.ttft_ms = 0;
            log.status_code = if self.saw_error { 502 } else { 200 };
            log.error_message = self.error_message.clone();
            log.finish_reason = if self.saw_error {
                "error".to_string()
            } else {
                crate::api::forward::non_streaming_finish_reason(&self.non_stream_body)
            };
            if self.non_stream_usage_authoritative {
                if let Some((prompt, completion, total)) =
                    crate::api::forward::exact_non_streaming_tokens(&self.non_stream_body)
                {
                    log.prompt_tokens = prompt;
                    log.completion_tokens = completion;
                    log.total_tokens = total;
                }
            }
        }
        if let Some(lease) = &self.key_lease {
            let failure = (log.status_code != 200).then(|| {
                if stream_error_status > 0 {
                    classify_status(stream_error_status)
                } else {
                    FailureInfo::new(FailureClass::Network)
                }
            });
            let config_current = self.winning.as_ref().is_some_and(|winning| {
                self.store
                    .channel_config_is_current(winning, &log.upstream_model)
                    .unwrap_or(false)
            });
            if upstream_failed
                && config_current
                && lease.is_current(&log.upstream_model).unwrap_or(false)
            {
                if let (Some(router), Some(winning)) = (&self.router, &self.winning) {
                    router.apply_live_outcome(
                        winning,
                        &log.upstream_model,
                        failure.as_ref(),
                        &log.error_message,
                        log.latency_ms,
                        self.outcome_started_at,
                    );
                }
            }
            if log.status_code == 200 && config_current {
                let _ = lease.success(
                    log.prompt_tokens,
                    log.completion_tokens,
                    &log.upstream_model,
                );
            } else if log.status_code != 200 && upstream_failed && config_current {
                if let Some(info) = failure {
                    let _ = lease.failure(&info, &log.upstream_model);
                }
            }
        }
        let mut attempt_ttft = 0;
        if self.is_stream {
            if let Some(mut measurement) = self.ttft.as_ref().and_then(|m| m.lock().ok()) {
                measurement.tracker.finish();
                if let Some(ttft_ms) = measurement.tracker.ttft_ms() {
                    attempt_ttft = ttft_ms;
                    // Logs retain client-perceived wait; routing samples exclude
                    // preceding failed attempts.
                    log.ttft_ms = measurement
                        .started
                        .saturating_duration_since(self.start)
                        .as_millis() as i64
                        + ttft_ms;
                    if log.status_code == 200 {
                        if let (Some(winning), Some(lease)) = (&self.winning, &self.key_lease) {
                            let observed_at =
                                measurement.started_at + chrono::Duration::milliseconds(ttft_ms);
                            let _ = self.store.record_live_model_ttft(
                                winning,
                                &log.upstream_model,
                                ttft_ms,
                                observed_at,
                                lease,
                                self.outcome_started_at,
                            );
                        }
                    }
                } else {
                    log.ttft_ms = 0;
                }
            }
        }
        if let Ok(mut trace) = serde_json::from_str::<Vec<TraceStep>>(&log.trace_json) {
            if let Some(step) = trace.last_mut() {
                step.ttft_ms = attempt_ttft;
                step.succeeded = log.status_code == 200;
                step.status_code = log.status_code;
                step.error = log.error_message.clone();
            }
            log.trace_json = serde_json::to_string(&trace).unwrap_or_default();
        }
        self.key_lease.take();
        self.recovery_claims.take();
        if log.id > 0 {
            let _ = self.store.update_log(&log);
        } else if let Some(winning) = &self.winning {
            log.channel_name = winning.name.clone();
            let _ = self.store.record_log(&mut log);
        }
    }
}

impl Stream for LoggingStream {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.finished {
            return Poll::Ready(None);
        }
        if this
            .deadline
            .as_mut()
            .is_some_and(|deadline| deadline.as_mut().poll(cx).is_ready())
        {
            this.finished = true;
            this.saw_error = true;
            this.error_message = "请求总时限已到".into();
            this.finalize();
            return Poll::Ready(Some(Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "request deadline exceeded",
            ))));
        }
        match this.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                let mut buffer_error = None;
                if this.is_stream {
                    if let Ok(mut tracker) = this.tracker.try_lock() {
                        tracker.feed(&chunk);
                        if tracker.buffer_exceeded {
                            buffer_error = Some(tracker.stream_error.clone());
                        }
                        if tracker.completion_tokens != this.last_reported
                            && this.last_update.elapsed() >= LOG_UPDATE_INTERVAL
                        {
                            if let Ok(mut log) = this.log.try_lock() {
                                log.completion_tokens = tracker.completion_tokens;
                                if tracker.prompt_tokens > 0 || tracker.has_exact_prompt_tokens {
                                    log.prompt_tokens = tracker.prompt_tokens;
                                }
                                log.total_tokens = log.prompt_tokens + log.completion_tokens;
                                log.latency_ms = this.start.elapsed().as_millis() as i64;
                                if log.id > 0 {
                                    let _ = this.store.update_log(&log);
                                }
                            }
                            this.last_update = Instant::now();
                            this.last_reported = tracker.completion_tokens;
                        }
                    }
                } else if this.non_stream_body.len() + chunk.len() <= MAX_REQUEST_BODY {
                    this.non_stream_body.extend_from_slice(&chunk);
                } else {
                    buffer_error = Some("upstream response body exceeded size limit".into());
                }
                if let Some(error) = buffer_error {
                    this.finished = true;
                    this.saw_error = true;
                    this.error_message = error.clone();
                    this.finalize();
                    return Poll::Ready(Some(Err(std::io::Error::other(error))));
                }
                Poll::Ready(Some(Ok(chunk)))
            }
            Poll::Ready(Some(Err(err))) => {
                this.finished = true;
                this.saw_error = true;
                this.error_message = err.to_string();
                this.finalize();
                Poll::Ready(Some(Err(err)))
            }
            Poll::Ready(None) => {
                if !this.finished {
                    this.finished = true;
                    this.finalize();
                }
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for LoggingStream {
    fn drop(&mut self) {
        if !self.finished {
            self.finished = true;
            self.client_cancelled = true;
            self.saw_error = true;
            self.error_message = "客户端取消或响应未完成".into();
            self.finalize();
        }
    }
}
