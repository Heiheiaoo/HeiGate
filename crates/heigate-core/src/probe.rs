//! Upstream health probing. Ported from the Go `internal/probe` package.
//!
//! A probe sends a tiny arithmetic question to a model and checks that the
//! answer comes back, which proves the endpoint, credentials, and model name
//! all work end to end. The whole (capped) stream is read so that an in-stream
//! error after the first delta is not mistaken for health. The arithmetic
//! answer is never validated — digits are often split across deltas.

#[path = "probe_inject.rs"]
pub mod inject;

pub use inject::{check_injection_model, check_injection_models, InjectionReport};

use std::collections::HashSet;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Provider identifiers. Persisted in the local database and exchanged with
/// the UI, so they must stay stable.
pub const PROVIDER_OPENAI: &str = "openai";
pub const PROVIDER_ANTHROPIC: &str = "anthropic";

pub const API_MODE_CHAT_COMPLETIONS: &str = "chat_completions";
pub const API_MODE_RESPONSES: &str = "responses";

/// A model answered within the latency threshold.
pub const STATUS_OPERATIONAL: &str = "operational";
/// A model answered but slowly.
pub const STATUS_DEGRADED: &str = "degraded";
/// A request reached the model but produced no content.
pub const STATUS_FAILED: &str = "failed";
/// A transport or HTTP error.
pub const STATUS_ERROR: &str = "error";

/// Bounds a single probe request, including body read.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
pub const MIN_INTERVAL_SECONDS: i64 = 15;
pub const MAX_INTERVAL_SECONDS: i64 = 86400;
pub const SCHEDULE_BUFFER: Duration = Duration::from_secs(10);

/// Deliberately tiny. Not 1 because some upstreams reject that outright; 16
/// clears every lower bound seen in the wild while keeping generation
/// negligible.
const MAX_ANSWER_TOKENS: i64 = 16;
pub(crate) const MAX_ERROR_SNIPPET: usize = 300;
pub(crate) const MAX_ERROR_BODY_READ: usize = 64 << 10;
const MAX_STREAM_BYTES: usize = 1 << 20;

/// The latency at or above which a correct answer is still reported degraded.
pub const DEGRADED_THRESHOLD: Duration = Duration::from_secs(6);

/// Validation errors. Messages contain "invalid" / "required" so HTTP handlers
/// can map them to 400 responses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeError {
    InvalidProvider,
    InvalidApiMode,
    InvalidEndpoint,
    InvalidInterval,
    MissingPrimaryModel,
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            ProbeError::InvalidProvider => {
                "invalid provider: expected \"openai\" or \"anthropic\""
            }
            ProbeError::InvalidApiMode => {
                "invalid api_mode: expected \"chat_completions\" or \"responses\" (responses is OpenAI only)"
            }
            ProbeError::InvalidEndpoint => {
                "invalid endpoint: expected an http:// or https:// URL"
            }
            ProbeError::InvalidInterval => {
                "invalid interval_seconds: expected 0 (manual) or a value between 15 and 86400"
            }
            ProbeError::MissingPrimaryModel => "primary_model is required",
        };
        write!(f, "{message}")
    }
}

impl std::error::Error for ProbeError {}

/// Describes one upstream channel to probe.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub api_mode: String,
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub primary_model: String,
    #[serde(default)]
    pub extra_models: Vec<String>,
    #[serde(default)]
    pub extra_headers: std::collections::HashMap<String, String>,
}

/// The outcome of probing a single model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeResult {
    #[serde(default)]
    pub http_status: i32,
    #[serde(default)]
    pub retry_after_secs: u64,
    #[serde(default)]
    pub key_id: i64,
    #[serde(default)]
    pub key_name: String,
    pub model: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttft_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ping_latency_ms: Option<i64>,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub message: String,
    pub checked_at: chrono::DateTime<chrono::Utc>,
}

impl ProbeResult {
    fn new(model: &str) -> Self {
        ProbeResult {
            http_status: 0,
            retry_after_secs: 0,
            key_id: 0,
            key_name: String::new(),
            model: model.to_string(),
            status: STATUS_ERROR.to_string(),
            latency_ms: None,
            ttft_ms: None,
            ping_latency_ms: None,
            message: String::new(),
            checked_at: chrono::Utc::now(),
        }
    }
}

/// The wall-clock a full run may need for `model_count` models: one attempt
/// each plus scheduling slack.
pub fn run_budget_for(model_count: i64) -> Duration {
    let model_count = model_count.max(1) as u32;
    Duration::from_secs(model_count as u64 * REQUEST_TIMEOUT.as_secs()) + SCHEDULE_BUFFER
}

/// Maps an empty mode to `chat_completions`.
pub fn normalize_api_mode(mode: &str) -> String {
    let mode = mode.trim();
    if mode.is_empty() {
        return API_MODE_CHAT_COMPLETIONS.to_string();
    }
    mode.to_string()
}

/// Trims whitespace, adds `https://` when no scheme is given, and removes a
/// trailing slash and a trailing `/v1` segment.
pub fn normalize_endpoint(raw: &str) -> String {
    let mut ep = raw.trim().to_string();
    if ep.is_empty() {
        return String::new();
    }
    if !ep.to_lowercase().contains("://") {
        ep = format!("https://{ep}");
    }
    ep = ep.trim_end_matches('/').to_string();
    if let Some(stripped) = ep.strip_suffix("/v1") {
        ep = stripped.to_string();
    }
    ep.trim_end_matches('/').to_string()
}

/// Removes credentials from an endpoint before it is written to request logs
/// or trace details. The live request still uses the original validated URL.
pub fn redact_endpoint(raw: &str) -> String {
    let Ok(mut url) = reqwest::Url::parse(raw) else {
        return raw.to_string();
    };
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(key, value)| {
            let lower = key.to_ascii_lowercase();
            let sensitive = ["token", "key", "secret", "password", "passwd", "auth"]
                .iter()
                .any(|name| lower.contains(name));
            (
                key.into_owned(),
                if sensitive {
                    "[REDACTED]".into()
                } else {
                    value.into_owned()
                },
            )
        })
        .collect();
    url.set_username("").ok();
    url.set_password(None).ok();
    if url.query().is_some() {
        url.set_query(None);
        if !pairs.is_empty() {
            let mut query = url.query_pairs_mut();
            query.extend_pairs(
                pairs
                    .iter()
                    .map(|(key, value)| (key.as_str(), value.as_str())),
            );
        }
    }
    url.set_fragment(None);
    url.to_string()
}

/// Checks provider, api mode, endpoint and primary model.
pub fn validate_config(cfg: &Config) -> Result<(), ProbeError> {
    let provider = cfg.provider.trim();
    if provider != PROVIDER_OPENAI && provider != PROVIDER_ANTHROPIC {
        return Err(ProbeError::InvalidProvider);
    }
    match normalize_api_mode(&cfg.api_mode).as_str() {
        API_MODE_CHAT_COMPLETIONS => {}
        API_MODE_RESPONSES => {
            if provider != PROVIDER_OPENAI {
                return Err(ProbeError::InvalidApiMode);
            }
        }
        _ => return Err(ProbeError::InvalidApiMode),
    }
    validate_endpoint(&cfg.endpoint)?;
    if cfg.primary_model.trim().is_empty() {
        return Err(ProbeError::MissingPrimaryModel);
    }
    Ok(())
}

/// Accepts 0 (manual probing only) or a value within the supported range.
pub fn validate_interval(seconds: i64) -> Result<(), ProbeError> {
    if seconds == 0 {
        return Ok(());
    }
    if !(MIN_INTERVAL_SECONDS..=MAX_INTERVAL_SECONDS).contains(&seconds) {
        return Err(ProbeError::InvalidInterval);
    }
    Ok(())
}

fn validate_endpoint(raw: &str) -> Result<(), ProbeError> {
    let ep = normalize_endpoint(raw);
    if ep.is_empty() {
        return Err(ProbeError::InvalidEndpoint);
    }
    match reqwest::Url::parse(&ep) {
        Ok(parsed) if !parsed.host_str().unwrap_or("").is_empty() => {
            if parsed.scheme() == "http" || parsed.scheme() == "https" {
                if !parsed.username().is_empty() || parsed.password().is_some() {
                    return Err(ProbeError::InvalidEndpoint);
                }
                Ok(())
            } else {
                Err(ProbeError::InvalidEndpoint)
            }
        }
        _ => Err(ProbeError::InvalidEndpoint),
    }
}

/// Probes the primary model and every extra model sequentially and returns one
/// result per distinct model, in configuration order. A non-`Ok` is only
/// returned for invalid configuration; upstream failures are per-model results.
pub async fn run(cfg: &Config) -> Result<Vec<ProbeResult>, ProbeError> {
    let mut cfg = cfg.clone();
    cfg.provider = cfg.provider.trim().to_string();
    cfg.api_mode = normalize_api_mode(&cfg.api_mode);
    cfg.endpoint = normalize_endpoint(&cfg.endpoint);
    cfg.api_key = cfg.api_key.trim().to_string();
    cfg.primary_model = cfg.primary_model.trim().to_string();
    validate_config(&cfg)?;

    let mut models = vec![cfg.primary_model.clone()];
    models.extend(cfg.extra_models.clone());
    let models = dedupe_models(&models);
    if models.is_empty() {
        return Err(ProbeError::MissingPrimaryModel);
    }

    let mut results = Vec::with_capacity(models.len());
    for model in models {
        results.push(check_model_once(&cfg, &model).await);
    }
    Ok(results)
}

/// Runs the lightweight channel health check for the primary model only.
pub async fn run_once(cfg: &Config) -> Result<Vec<ProbeResult>, ProbeError> {
    run(cfg).await
}

pub(crate) fn dedupe_models(models: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for model in models {
        let model = model.trim();
        if model.is_empty() {
            continue;
        }
        if seen.insert(model.to_string()) {
            out.push(model.to_string());
        }
    }
    out
}

/// A bare sum: reasoning models spend far longer on an English instruction than
/// on plain arithmetic, and the answer is never validated anyway.
fn new_challenge() -> String {
    let a = random_int(10, 89);
    let b = random_int(10, 89);
    format!("{a} + {b} = ?")
}

fn random_int(lo: i64, hi: i64) -> i64 {
    use getrandom::fill;
    let span = (hi - lo + 1) as u64;
    let mut buf = [0u8; 8];
    if fill(&mut buf).is_err() {
        let nanos = chrono::Utc::now()
            .timestamp_nanos_opt()
            .unwrap_or(0)
            .unsigned_abs();
        return lo + (nanos % span) as i64;
    }
    lo + (u64::from_le_bytes(buf) % span) as i64
}

/// Performs a single challenge request.
pub async fn check_model_once(cfg: &Config, model: &str) -> ProbeResult {
    let mut res = ProbeResult::new(model);
    let prompt = new_challenge();

    let started = Instant::now();
    let outcome = send(cfg, model, &prompt).await;
    let mut latency_ms = started.elapsed().as_millis() as i64;

    let out = match outcome {
        Ok(out) => out,
        Err(err) => {
            res.message = redact(&truncate(&err.to_string(), MAX_ERROR_SNIPPET), &cfg.api_key);
            res.latency_ms = Some(latency_ms);
            return res;
        }
    };

    res.http_status = out.status;
    res.retry_after_secs = out.retry_after_secs;
    if let Some(ttft) = out.ttft {
        let ttft_ms = ttft.as_millis() as i64;
        res.ttft_ms = Some(ttft_ms);
        // Expose the first-token measurement as LatencyMs so every UI surface
        // reports one consistent probe time.
        latency_ms = ttft_ms;
    }
    res.latency_ms = Some(latency_ms);

    // Any non-2xx (5xx included) reports the upstream status and body. The
    // caller decides when to take the next independent sample.
    if !(200..300).contains(&out.status) {
        res.message = redact(
            &format!(
                "upstream HTTP {}: {}",
                out.status,
                truncate(&compact_whitespace(&out.body), MAX_ERROR_SNIPPET)
            ),
            &cfg.api_key,
        );
    } else if !out.saw_content && !out.saw_reasoning {
        // 2xx but nothing was ever produced.
        res.status = STATUS_FAILED.to_string();
        res.message = "upstream returned 2xx but no content".to_string();
    } else if Duration::from_millis(latency_ms as u64) >= DEGRADED_THRESHOLD {
        res.status = STATUS_DEGRADED.to_string();
        res.message = format!("slow response: {latency_ms}ms");
    } else {
        res.status = STATUS_OPERATIONAL.to_string();
    }
    res
}

/// What a probe request learned. `body` is only meaningful for non-2xx.
#[derive(Debug, Default)]
struct SendResult {
    retry_after_secs: u64,
    ttft: Option<Duration>,
    saw_content: bool,
    saw_reasoning: bool,
    status: i32,
    body: String,
}

/// Performs one streaming provider request. The first visible text delta is
/// recorded as TTFT, falling back to the first reasoning delta.
async fn send(cfg: &Config, model: &str, prompt: &str) -> Result<SendResult, String> {
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|err| err.to_string())?;

    let path;
    let payload: Value;
    match cfg.provider.as_str() {
        PROVIDER_ANTHROPIC => {
            path = "/v1/messages";
            payload = json!({
                "model": model,
                "max_tokens": MAX_ANSWER_TOKENS,
                "messages": [{"role": "user", "content": prompt}],
                "stream": true,
            });
        }
        PROVIDER_OPENAI => {
            if cfg.api_mode == API_MODE_RESPONSES {
                path = "/v1/responses";
                payload = json!({
                    "model": model,
                    "input": prompt,
                    "max_output_tokens": MAX_ANSWER_TOKENS,
                    "stream": true,
                });
            } else {
                path = "/v1/chat/completions";
                payload = json!({
                    "model": model,
                    "messages": [{"role": "user", "content": prompt}],
                    "max_tokens": MAX_ANSWER_TOKENS,
                    "stream": true,
                });
            }
        }
        _ => return Err(ProbeError::InvalidProvider.to_string()),
    }

    let url = format!("{}{}", cfg.endpoint, path);
    let mut request = client
        .post(&url)
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .json(&payload);

    match cfg.provider.as_str() {
        PROVIDER_ANTHROPIC => {
            if !cfg.api_key.is_empty() {
                request = request.header("x-api-key", &cfg.api_key);
            }
            request = request.header("anthropic-version", "2023-06-01");
        }
        PROVIDER_OPENAI if !cfg.api_key.is_empty() => {
            request = request.header("authorization", format!("Bearer {}", cfg.api_key));
        }
        _ => {}
    }
    for (key, value) in &cfg.extra_headers {
        if !key.trim().is_empty() {
            request = request.header(key, value);
        }
    }

    // Measured from before the request goes out, as the Go probe does: the
    // client-perceived wait is connect + upstream thinking time + first byte,
    // not just the span between the response headers and the first delta (which
    // is sub-millisecond, and rounds to a zero that reads as "not measured").
    let started = Instant::now();
    let response = request.send().await.map_err(|err| err.to_string())?;
    let status = response.status().as_u16() as i32;

    if !(200..300).contains(&status) {
        let retry_after_secs = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .map(crate::api::forward::parse_retry_after)
            .unwrap_or_default()
            .as_secs();
        let body = response.text().await.unwrap_or_default();
        let body: String = body.chars().take(MAX_ERROR_BODY_READ).collect();
        return Ok(SendResult {
            retry_after_secs,
            status,
            body,
            ..Default::default()
        });
    }

    read_stream(cfg, response, started).await
}

/// Consumes the SSE stream and accumulates the answer. Text and reasoning are
/// tracked separately so TTFT can prefer visible text.
async fn read_stream(
    cfg: &Config,
    response: reqwest::Response,
    started: Instant,
) -> Result<SendResult, String> {
    let out = SendResult {
        status: 200,
        ..Default::default()
    };
    let mut text_at: Option<Instant> = None;
    let mut reason_at: Option<Instant> = None;

    let mut stream = response.bytes_stream();
    let mut buffer = Vec::<u8>::new();
    let mut total = 0usize;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|err| err.to_string())?;
        total += chunk.len();
        if total > MAX_STREAM_BYTES {
            break;
        }
        buffer.extend_from_slice(&chunk);

        // Process complete lines from the buffer.
        loop {
            let newline = buffer.iter().position(|b| *b == b'\n');
            let Some(newline) = newline else { break };
            let line_bytes: Vec<u8> = buffer.drain(..=newline).collect();
            let line = String::from_utf8_lossy(&line_bytes);
            let (payload, done, ok) = parse_sse_line(&line);
            if !ok {
                continue;
            }
            if done {
                return finish_stream(out, text_at, reason_at, started);
            }

            // An error object inside a 200 SSE stream is still an error.
            if let Some(message) = stream_error(&payload) {
                return Err(format!(
                    "upstream stream error: {}",
                    compact_whitespace(&message)
                ));
            }

            apply_delta(cfg, &payload, &mut text_at, &mut reason_at);
        }
    }

    finish_stream(out, text_at, reason_at, started)
}

/// Records what the stream proved — that content arrived, and when — without
/// keeping the answer: a probe only needs to show the path works.
fn finish_stream(
    mut out: SendResult,
    text_at: Option<Instant>,
    reason_at: Option<Instant>,
    started: Instant,
) -> Result<SendResult, String> {
    out.saw_content = text_at.is_some();
    out.saw_reasoning = reason_at.is_some();
    if let Some(at) = text_at {
        out.ttft = Some(at.duration_since(started));
    } else if let Some(at) = reason_at {
        out.ttft = Some(at.duration_since(started));
    }
    Ok(out)
}

fn stream_error(payload: &str) -> Option<String> {
    let value: Value = serde_json::from_str(payload).ok()?;
    let error = value.get("error")?;
    if error.is_null() {
        return None;
    }
    if let Some(message) = error.get("message").and_then(Value::as_str) {
        if !message.is_empty() {
            return Some(message.to_string());
        }
    }
    if let Some(text) = error.as_str() {
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }
    Some(error.to_string())
}

fn apply_delta(
    cfg: &Config,
    payload: &str,
    text_at: &mut Option<Instant>,
    reason_at: &mut Option<Instant>,
) {
    let Ok(value) = serde_json::from_str::<Value>(payload) else {
        return;
    };

    if cfg.provider == PROVIDER_ANTHROPIC {
        if value.get("type").and_then(Value::as_str) != Some("content_block_delta") {
            return;
        }
        let delta = value.get("delta");
        let kind = delta.and_then(|d| d.get("type")).and_then(Value::as_str);
        match kind {
            Some("text_delta") => {
                if let Some(part) = delta.and_then(|d| d.get("text")).and_then(Value::as_str) {
                    if !part.is_empty() {
                        text_at.get_or_insert_with(Instant::now);
                    }
                }
            }
            Some("thinking_delta") => {
                if let Some(part) = delta
                    .and_then(|d| d.get("thinking"))
                    .and_then(Value::as_str)
                {
                    if !part.is_empty() {
                        reason_at.get_or_insert_with(Instant::now);
                    }
                }
            }
            _ => {}
        }
        return;
    }

    if cfg.api_mode == API_MODE_RESPONSES {
        let kind = value.get("type").and_then(Value::as_str);
        let delta = value.get("delta").and_then(Value::as_str).unwrap_or("");
        match kind {
            Some("response.output_text.delta") => {
                if !delta.is_empty() {
                    text_at.get_or_insert_with(Instant::now);
                }
            }
            Some("response.reasoning_text.delta") if !delta.is_empty() => {
                reason_at.get_or_insert_with(Instant::now);
            }
            _ => {}
        }
        return;
    }

    // OpenAI chat completions.
    let Some(choices) = value.get("choices").and_then(Value::as_array) else {
        return;
    };
    for choice in choices {
        let Some(delta) = choice.get("delta") else {
            continue;
        };
        let part = content_to_text(delta.get("content"));
        if !part.is_empty() {
            text_at.get_or_insert_with(Instant::now);
        }
        let thinking = ["reasoning_content", "reasoning", "thinking", "thought"]
            .iter()
            .find_map(|key| delta.get(*key).and_then(Value::as_str))
            .unwrap_or("");
        if !thinking.is_empty() {
            reason_at.get_or_insert_with(Instant::now);
        }
    }
}

/// Extracts the JSON payload from a `data: ...` line.
pub fn parse_sse_line(line: &str) -> (String, bool, bool) {
    let line = line.trim();
    let Some(rest) = line.strip_prefix("data:") else {
        return (String::new(), false, false);
    };
    let payload = rest.trim();
    if payload == "[DONE]" {
        return (String::new(), true, true);
    }
    if payload.is_empty() {
        return (String::new(), false, false);
    }
    (payload.to_string(), false, true)
}

/// Handles both string content and an array of content parts.
pub(crate) fn content_to_text(raw: Option<&Value>) -> String {
    match raw {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => {
            let mut out = String::new();
            for part in parts {
                let kind = part.get("type").and_then(Value::as_str).unwrap_or("");
                if kind.is_empty() || kind == "text" || kind == "output_text" {
                    if let Some(text) = part.get("text").and_then(Value::as_str) {
                        out.push_str(text);
                    }
                }
            }
            out
        }
        Some(_) => String::new(),
    }
}

pub(crate) fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max).collect();
    format!("{truncated}…")
}

pub(crate) fn compact_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Hides the API key if a provider echoes it back in an error body.
fn redact(s: &str, secret: &str) -> String {
    if secret.is_empty() || secret.len() < 6 {
        return s.to_string();
    }
    s.replace(secret, "***")
}

#[cfg(test)]
mod tests {
    //! Ports the pure-logic cases from `internal/probe/probe_test.go`.

    use super::*;

    #[test]
    fn normalize_endpoint_strips_scheme_and_v1() {
        assert_eq!(
            normalize_endpoint("https://api.example.com/v1/"),
            "https://api.example.com"
        );
        assert_eq!(
            normalize_endpoint("api.example.com"),
            "https://api.example.com"
        );
        assert_eq!(normalize_endpoint("  https://a.b  "), "https://a.b");
        assert_eq!(normalize_endpoint(""), "");
    }

    #[test]
    fn endpoint_redaction_hides_query_credentials() {
        let redacted = redact_endpoint("https://api.example.com/v1?token=secret&api-version=2024");
        assert!(redacted.contains("token=%5BREDACTED%5D"));
        assert!(redacted.contains("api-version=2024"));
        assert!(!redacted.contains("secret"));
    }

    #[test]
    fn endpoint_validation_rejects_url_credentials() {
        let cfg = Config {
            provider: PROVIDER_OPENAI.to_string(),
            api_mode: API_MODE_CHAT_COMPLETIONS.to_string(),
            endpoint: "https://user:password@example.com".to_string(),
            primary_model: "gpt-4o".to_string(),
            ..Default::default()
        };
        assert_eq!(validate_config(&cfg), Err(ProbeError::InvalidEndpoint));
    }

    #[test]
    fn validate_config_enforces_provider_and_mode() {
        let good = Config {
            provider: PROVIDER_OPENAI.to_string(),
            api_mode: API_MODE_CHAT_COMPLETIONS.to_string(),
            endpoint: "https://api.example.com".to_string(),
            primary_model: "gpt-4o".to_string(),
            ..Default::default()
        };
        assert_eq!(validate_config(&good), Ok(()));

        let mut bad_provider = good.clone();
        bad_provider.provider = "gemini".to_string();
        assert_eq!(
            validate_config(&bad_provider),
            Err(ProbeError::InvalidProvider)
        );

        let mut anthropic_responses = good.clone();
        anthropic_responses.provider = PROVIDER_ANTHROPIC.to_string();
        anthropic_responses.api_mode = API_MODE_RESPONSES.to_string();
        assert_eq!(
            validate_config(&anthropic_responses),
            Err(ProbeError::InvalidApiMode)
        );

        let mut no_model = good.clone();
        no_model.primary_model = "  ".to_string();
        assert_eq!(
            validate_config(&no_model),
            Err(ProbeError::MissingPrimaryModel)
        );
    }

    #[test]
    fn interval_validation() {
        assert_eq!(validate_interval(0), Ok(()));
        assert_eq!(validate_interval(15), Ok(()));
        assert_eq!(validate_interval(86400), Ok(()));
        assert_eq!(validate_interval(14), Err(ProbeError::InvalidInterval));
        assert_eq!(validate_interval(86401), Err(ProbeError::InvalidInterval));
    }

    #[test]
    fn run_budget_scales_with_models() {
        assert_eq!(run_budget_for(1), REQUEST_TIMEOUT + SCHEDULE_BUFFER);
        assert_eq!(run_budget_for(3), REQUEST_TIMEOUT * 3 + SCHEDULE_BUFFER);
        assert_eq!(run_budget_for(0), REQUEST_TIMEOUT + SCHEDULE_BUFFER);
    }

    #[test]
    fn challenge_is_a_two_digit_sum() {
        for _ in 0..20 {
            let prompt = new_challenge();
            let parts: Vec<&str> = prompt.split(" + ").collect();
            assert_eq!(parts.len(), 2);
            let a: i64 = parts[0].parse().unwrap();
            let b: i64 = parts[1].trim_end_matches(" = ?").parse().unwrap();
            assert!((10..=89).contains(&a));
            assert!((10..=89).contains(&b));
        }
    }

    #[test]
    fn dedupe_and_helpers() {
        assert_eq!(
            dedupe_models(&[
                " a ".to_string(),
                "a".to_string(),
                "b".to_string(),
                "".to_string()
            ]),
            vec!["a".to_string(), "b".to_string()]
        );
        assert_eq!(compact_whitespace("a\n  b\tc"), "a b c");
        assert_eq!(truncate("hello", 3), "hel…");
        assert_eq!(redact("key sk-secret key", "sk-secret"), "key *** key");
        assert_eq!(redact("no secret here", "short"), "no secret here");
    }

    #[test]
    fn parse_sse_line_basics() {
        assert_eq!(parse_sse_line("data: {}"), ("{}".to_string(), false, true));
        assert_eq!(parse_sse_line("data: [DONE]"), (String::new(), true, true));
        assert_eq!(parse_sse_line(": ping"), (String::new(), false, false));
    }

    #[test]
    fn stream_error_detection() {
        assert!(stream_error("{\"choices\":[]}").is_none());
        assert_eq!(
            stream_error("{\"error\":{\"message\":\"quota\"}}"),
            Some("quota".to_string())
        );
        assert_eq!(
            stream_error("{\"error\":\"boom\"}"),
            Some("boom".to_string())
        );
        assert!(stream_error("{\"error\":null}").is_none());
    }
}
