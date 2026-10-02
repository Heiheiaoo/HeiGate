//! The gateway endpoints' forwarding core: candidate selection, the first-byte
//! failover loop, protocol translation, and the client-facing stream with
//! request-log bookkeeping. Ported from `internal/api/gateway.go`.

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
    parse_retry_after, request_model, GatewayProtocol, StreamTokenTracker,
};
use crate::api::policy::{
    adapt_unsupported_reasoning_effort, best_auto_choice, best_auto_model, channel_serving_traffic,
    channel_supports_model, parse_upstream_error_message, upstream_error_payload,
    upstream_stream_error, upstream_stream_has_output, TraceStep, AUTO_MODEL_NAME,
};
use crate::canonical::find_channel_model_for_request;
use crate::key_pool::{KeyLease, KeyPool};
use crate::probe::{normalize_endpoint, PROVIDER_OPENAI};
use crate::protocol::{
    chat_to_messages, encode_sse, messages_to_chat, parse_sse_line, ChatChunk, MessagesRequest,
    StreamState,
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

/// A streaming byte source for an upstream response (or a synthesized body).
pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>>;

/// An upstream reply before it is handed to the client.
pub struct UpstreamRaw {
    pub status: u16,
    pub headers: HeaderMap,
    pub stream: ByteStream,
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
            GatewayError::Other(message) => write!(f, "{message}"),
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

        let matcher = |ch: &Channel| gateway_candidate_matcher(proto, ch);
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
            candidates.sort_by(|a, b| {
                let ca = best_auto_choice(Some(a));
                let cb = best_auto_choice(Some(b));
                ca.rank
                    .cmp(&cb.rank)
                    .then(ca.lat.cmp(&cb.lat))
                    .then(a.name.cmp(&b.name))
            });
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

        let mut chosen: Option<(UpstreamRaw, Channel, i64, KeyLease)> = None;
        let mut last_error: Option<GatewayError> = None;
        let mut claimed = RecoveryClaims {
            router: self.router.clone(),
            ids: Vec::new(),
        };
        let deadline = tokio::time::Instant::now() + Duration::from_secs(600);
        let mut attempts = 0;
        let mut blocked_retry = Duration::ZERO;

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
            let upstream_model = if is_auto {
                best_auto_model(Some(channel))
            } else {
                find_channel_model_for_request(channel, &effective_model)
                    .map(|(actual, _)| actual)
                    .filter(|m| !m.is_empty())
                    .unwrap_or_else(|| effective_model.clone())
            };
            if upstream_model.is_empty() {
                continue;
            }
            let mut excluded = std::collections::HashSet::new();
            for _ in 0..2 {
                if attempts >= 3 {
                    break 'candidates;
                }
                let lease =
                    match self
                        .key_pool
                        .acquire(channel.id, &upstream_model, &excluded, None)
                    {
                        Ok(Some(lease)) => lease,
                        Ok(None) => {
                            let wait = self.key_pool.retry_after(channel.id);
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
                // A per-attempt snapshot carries the selected credential. The stored channel is never mutated.
                let mut selected = channel.clone();
                selected.api_key = lease.key.secret.clone();
                let attempt_start = Instant::now();
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
                    .unwrap_or_else(|_| Err(GatewayError::Other("total upstream timeout".into())));
                    let latency = attempt_start.elapsed().as_millis() as i64;
                    let mut step = TraceStep {
                        index: trace_steps.len() as i64 + 1,
                        channel_id: channel.id,
                        channel_name: channel.name.clone(),
                        key_id: lease.key.id,
                        key_name: lease.key.name.clone(),
                        endpoint: channel.endpoint.clone(),
                        model: upstream_model.clone(),
                        attempt: attempts,
                        latency_ms: latency,
                        ..Default::default()
                    };
                    match outcome {
                        Ok(upstream) => {
                            step.status_code = upstream.status as i64;
                            step.ttft_ms = latency;
                            step.succeeded = true;
                            trace_steps.push(step);
                            self.router.apply_channel_success(channel, latency);
                            chosen = Some((
                                upstream,
                                selected,
                                start_overall.elapsed().as_millis() as i64,
                                lease,
                            ));
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
                            let _ = lease.failure(&info, &upstream_model);
                            let key_fault = matches!(
                                info.class,
                                Some(
                                    FailureClass::RateLimit
                                        | FailureClass::Auth
                                        | FailureClass::Model
                                )
                            );
                            if !key_fault {
                                self.penalize(channel, &err, &message, latency);
                            }
                            let client_fault = info.class == Some(FailureClass::Client);
                            last_error = Some(err);
                            if client_fault {
                                break 'candidates;
                            }
                            if !key_fault {
                                continue 'candidates;
                            }
                            break;
                        }
                    }
                }
            }
        }
        if chosen.is_none() {
            // Include leases which just entered cooldown, not only channels initially exhausted.
            for channel in &candidates {
                let wait = self.key_pool.retry_after(channel.id);
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
        drop(claimed);

        let total_latency = start_overall.elapsed().as_millis() as i64;

        let Some((upstream, winning, winning_ttft, key_lease)) = chosen else {
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
            winning_ttft,
            is_stream_request,
            &failed_attempts,
            &trace_steps,
            request_log,
            start_overall,
            ctx,
            total_latency,
            key_lease,
        )
        .await
    }

    /// Records a channel-fault failure against the channel so the circuit
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
        let mut upstream = self
            .forward_upstream(inbound, body, channel, proto, client_model, upstream_model)
            .await?;
        let is_sse = header_contains(&upstream.headers, "content-type", "text/event-stream");
        let (prefix, rest, read_err) = read_initial_prefix(
            upstream.stream,
            is_sse && (200..300).contains(&upstream.status),
        )
        .await;
        if !(200..300).contains(&upstream.status) {
            return Err(GatewayError::UpstreamStatus {
                status: upstream.status as i32,
                retry_after: retry_after_from(&upstream.headers),
                message: parse_upstream_error_message(&String::from_utf8_lossy(&prefix)),
            });
        }
        if prefix.is_empty() {
            return Err(GatewayError::StreamStart {
                message: read_err
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "empty response".into()),
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
        }
        upstream.stream =
            Box::pin(stream::once(async move { Ok(Bytes::from(prefix)) }).chain(rest));
        Ok(upstream)
    }

    /// breaker and cooldown engage, mirroring what `router::route_candidates`
    /// does for the probe path. Client-class failures are the request's fault
    /// and never bench the channel.
    fn penalize(&self, channel: &mut Channel, err: &GatewayError, message: &str, latency: i64) {
        let info = err.failure_info();
        if crate::resilience::failure_is_channel_fault(&info) {
            self.router
                .apply_channel_failure(channel, &info, message, latency);
        }
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
                    && proto != GatewayProtocol::Anthropic
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
        let mut msg_req: MessagesRequest = serde_json::from_slice(body)
            .map_err(|err| GatewayError::Other(format!("parse anthropic request: {err}")))?;
        if !client_model.is_empty() {
            msg_req.model = client_model.to_string();
        }
        let mut chat_req = messages_to_chat(Some(&msg_req))
            .map_err(|err| GatewayError::Other(format!("convert anthropic request: {err}")))?;
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
            let body_text = response.text().await.unwrap_or_default();
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
            });
        }

        if !msg_req.stream {
            let resp_bytes = response
                .bytes()
                .await
                .map_err(|err| GatewayError::Other(format!("read chat response: {err}")))?;
            if let Some((message, err_status)) =
                upstream_error_payload(&String::from_utf8_lossy(&resp_bytes))
            {
                return Err(GatewayError::upstream_status(err_status, message));
            }
            let chat_resp: crate::protocol::ChatResponse = serde_json::from_slice(&resp_bytes)
                .map_err(|err| GatewayError::Other(format!("decode chat response: {err}")))?;
            let messages = chat_to_messages(&chat_resp, &msg_req.model);
            let out = serde_json::to_vec(&messages)
                .map_err(|err| GatewayError::Other(format!("encode anthropic response: {err}")))?;
            let mut headers = HeaderMap::new();
            headers.insert("content-type", HeaderValue::from_static("application/json"));
            return Ok(UpstreamRaw {
                status: 200,
                headers,
                stream: single_chunk(out),
            });
        }

        // Streaming: translate each OpenAI chunk into Anthropic events.
        let model = msg_req.model.clone();
        let upstream_stream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>> =
            Box::pin(response.bytes_stream());
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
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn deliver(
        &self,
        _proto: GatewayProtocol,
        upstream: UpstreamRaw,
        winning: Channel,
        winning_ttft: i64,
        is_stream_request: bool,
        failed_attempts: &[String],
        trace_steps: &[TraceStep],
        mut request_log: RequestLog,
        start: Instant,
        ctx: &RequestContext,
        total_latency: i64,
        key_lease: KeyLease,
    ) -> Response {
        let is_stream = is_stream_request
            || header_contains(&upstream.headers, "content-type", "text/event-stream");
        let failover_from = failed_attempts.join(" → ");

        if request_log.id > 0 {
            request_log.channel_name = winning.name.clone();
            request_log.endpoint = winning.endpoint.clone();
            request_log.is_stream = is_stream;
            request_log.is_failover = !failed_attempts.is_empty();
            request_log.failover_from = failover_from.clone();
            if request_log.ttft_ms == 0 {
                request_log.ttft_ms = winning_ttft;
            }
            request_log.trace_json = serde_json::to_string(trace_steps).unwrap_or_default();
            let _ = self.store.update_log(&request_log);
        }

        let tracker = Arc::new(Mutex::new(StreamTokenTracker::new(
            request_log.prompt_tokens,
        )));
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
        request_log.latency_ms = total_latency;
        request_log.ttft_ms = total_latency;
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
                "api_error",
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
    }
}

fn single_chunk(data: Vec<u8>) -> ByteStream {
    Box::pin(stream::once(async move { Ok(Bytes::from(data)) }))
}

/// Reads the initial bytes of an upstream body so the caller can fail over
/// before any byte reaches the client.
async fn read_initial_prefix(
    mut stream: ByteStream,
    is_sse: bool,
) -> (Vec<u8>, ByteStream, Option<std::io::Error>) {
    let mut prefix: Vec<u8> = Vec::new();
    if !is_sse {
        // Non-SSE: read at least one byte, up to 4096.
        match stream.next().await {
            Some(Ok(chunk)) => {
                prefix.extend_from_slice(&chunk);
                while prefix.len() < 4096 {
                    match stream.next().await {
                        Some(Ok(chunk)) => prefix.extend_from_slice(&chunk),
                        _ => break,
                    }
                }
                (prefix, stream, None)
            }
            Some(Err(err)) => (prefix, stream, Some(err)),
            None => (prefix, stream, None),
        }
    } else {
        while prefix.len() < MAX_STREAM_PREFIX_BYTES {
            match stream.next().await {
                Some(Ok(chunk)) => {
                    prefix.extend_from_slice(&chunk);
                    let text = String::from_utf8_lossy(&prefix);
                    let has_boundary = text.contains("\n\n") || text.contains("\r\n\r\n");
                    if has_boundary
                        && (upstream_stream_error(&text).is_some()
                            || upstream_stream_has_output(&text))
                    {
                        return (prefix, stream, None);
                    }
                }
                Some(Err(err)) => return (prefix, stream, Some(err)),
                None => break,
            }
        }
        (prefix, stream, None)
    }
}

/// Converts an OpenAI SSE byte stream into an Anthropic SSE byte stream.
fn translated_stream(
    upstream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    model: String,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send {
    struct State {
        upstream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
        buffer: String,
        state: StreamState,
        finished: bool,
        pending: std::collections::VecDeque<Result<Bytes, std::io::Error>>,
        saw_done: bool,
        saw_finish: bool,
    }

    stream::unfold(
        State {
            upstream,
            buffer: String::new(),
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
                        st.buffer.push_str(&String::from_utf8_lossy(&chunk));
                        while let Some(newline) = st.buffer.find('\n') {
                            let line: String = st.buffer.drain(..=newline).collect();
                            let (payload, done, ok) = parse_sse_line(&line);
                            if !ok {
                                continue;
                            }
                            if done {
                                st.saw_done = true;
                                break;
                            }
                            if upstream_error_payload(&payload).is_some() {
                                st.finished = true;
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
                            for event in events {
                                if let Ok(frame) = encode_sse(&event) {
                                    st.pending.push_back(Ok(Bytes::from(frame)));
                                }
                            }
                        }
                        if st.saw_done {
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
    key_lease: Option<KeyLease>,
    non_stream_body: Vec<u8>,
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
            key_lease: None,
            non_stream_body: Vec::new(),
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
        ) = snapshot;
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
            if prompt_tokens > 0 {
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
            } else if saw_done || !finish_reason.is_empty() {
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
            log.status_code = if self.saw_error { 502 } else { 200 };
            log.finish_reason = if self.saw_error { "error" } else { "stop" }.to_string();
            let (prompt, completion, total) =
                crate::api::forward::extract_non_streaming_tokens(&self.non_stream_body);
            if prompt > 0 || completion > 0 {
                log.prompt_tokens = prompt;
                log.completion_tokens = completion;
                log.total_tokens = total;
            }
        }
        if let Some(lease) = self.key_lease.take() {
            if log.status_code == 200 {
                let _ = lease.success(log.prompt_tokens, log.completion_tokens);
            } else if stream_error_status > 0 {
                let _ = lease.failure(&classify_status(stream_error_status), &log.model);
            }
        }
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
        match this.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                if this.is_stream {
                    if let Ok(mut tracker) = this.tracker.try_lock() {
                        tracker.feed(&chunk);
                        if tracker.completion_tokens != this.last_reported
                            && this.last_update.elapsed() >= LOG_UPDATE_INTERVAL
                        {
                            if let Ok(mut log) = this.log.try_lock() {
                                log.completion_tokens = tracker.completion_tokens;
                                if tracker.prompt_tokens > 0 {
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
                }
                Poll::Ready(Some(Ok(chunk)))
            }
            Poll::Ready(Some(Err(err))) => {
                this.saw_error = true;
                this.error_message = err.to_string();
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
