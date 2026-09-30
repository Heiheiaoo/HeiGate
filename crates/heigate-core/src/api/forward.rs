//! Gateway forwarding primitives that carry no HTTP plumbing: the streaming
//! token tracker, request parsing, token estimation, and header policy.
//! Ported from `internal/api/gateway.go`.

use std::time::Duration;

use serde_json::Value;

use crate::protocol::estimate_tokens;

/// Which wire protocol the client is speaking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayProtocol {
    Anthropic,
    Chat,
    Responses,
}

/// Tracks token usage across a proxied stream. Ported from the Go
/// `streamTokenTracker`.
#[derive(Debug, Default)]
pub struct StreamTokenTracker {
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub total_tokens: i64,
    pub has_exact_tokens: bool,
    pub estimated_chars: i64,
    pub leftover: String,
    pub saw_done: bool,
    pub finish_reason: String,
    pub stream_error: String,
    pub stream_error_status: i32,
}

impl StreamTokenTracker {
    pub fn new(prompt_tokens: i64) -> Self {
        StreamTokenTracker {
            prompt_tokens,
            ..Default::default()
        }
    }

    /// Feeds one chunk of raw bytes, buffering a partial trailing line.
    pub fn feed(&mut self, chunk: &[u8]) {
        if chunk.is_empty() {
            return;
        }
        let text = format!("{}{}", self.leftover, String::from_utf8_lossy(chunk));
        let mut lines: Vec<&str> = text.split('\n').collect();
        self.leftover = lines.pop().unwrap_or("").to_string();

        for line in lines {
            let line = line.trim();
            let Some(rest) = line.strip_prefix("data:") else {
                continue;
            };
            let payload = rest.trim();
            if payload == "[DONE]" {
                self.saw_done = true;
                continue;
            }
            if payload.is_empty() {
                continue;
            }

            let Ok(generic) = serde_json::from_str::<Value>(payload) else {
                continue;
            };
            if let Some((message, status)) = crate::api::upstream_error_payload(payload) {
                self.stream_error = message;
                self.stream_error_status = status;
                continue;
            }

            // finish_reason in choices.
            if let Some(finish) = generic
                .get("choices")
                .and_then(Value::as_array)
                .and_then(|choices| choices.first())
                .and_then(|choice| choice.get("finish_reason"))
                .and_then(Value::as_str)
            {
                if !finish.is_empty() {
                    self.finish_reason = finish.to_string();
                    if matches!(finish, "stop" | "length" | "tool_calls") {
                        self.saw_done = true;
                    }
                }
            }

            let event_type = generic.get("type").and_then(Value::as_str).unwrap_or("");
            // Anthropic message_stop, and the Responses API's response.completed,
            // are authoritative completion markers.
            if event_type == "message_stop" || event_type == "response.completed" {
                self.saw_done = true;
                if self.finish_reason.is_empty() {
                    self.finish_reason = "stop".to_string();
                }
            }

            // 1. Standard OpenAI usage.
            if let Some(usage) = generic.get("usage").filter(|u| !u.is_null()) {
                let p = get_int_field(usage, &["prompt_tokens", "input_tokens"]);
                let c = get_int_field(usage, &["completion_tokens", "output_tokens"]);
                let tot = get_int_field(usage, &["total_tokens"]);
                if p > 0 {
                    self.prompt_tokens = p;
                }
                if c > 0 {
                    self.completion_tokens = c;
                }
                if tot > 0 {
                    self.total_tokens = tot;
                } else if self.prompt_tokens > 0 || self.completion_tokens > 0 {
                    self.total_tokens = self.prompt_tokens + self.completion_tokens;
                }
                self.has_exact_tokens = true;
                continue;
            }

            // 2. Anthropic message_start usage.
            if event_type == "message_start" {
                if let Some(usage) = generic
                    .get("message")
                    .and_then(|m| m.get("usage"))
                {
                    let p = get_int_field(usage, &["input_tokens", "prompt_tokens"]);
                    if p > 0 {
                        self.prompt_tokens = p;
                    }
                }
                continue;
            }

            // 3. Anthropic message_delta usage.
            if event_type == "message_delta" {
                if let Some(usage) = generic.get("usage") {
                    let c = get_int_field(usage, &["output_tokens", "completion_tokens"]);
                    if c > 0 {
                        self.completion_tokens = c;
                        self.has_exact_tokens = true;
                    }
                }
                continue;
            }

            // 4. In-flight estimate from streaming text.
            if !self.has_exact_tokens {
                let mut delta_content = String::new();
                if let Some(delta) = generic
                    .get("choices")
                    .and_then(Value::as_array)
                    .and_then(|choices| choices.first())
                    .and_then(|choice| choice.get("delta"))
                {
                    if let Some(c) = delta.get("content").and_then(Value::as_str) {
                        delta_content.push_str(c);
                    }
                    if let Some(r) = delta.get("reasoning_content").and_then(Value::as_str) {
                        delta_content.push_str(r);
                    }
                }
                if event_type == "content_block_delta" {
                    if let Some(delta) = generic.get("delta") {
                        if let Some(t) = delta.get("text").and_then(Value::as_str) {
                            delta_content.push_str(t);
                        }
                        if let Some(t) = delta.get("thinking").and_then(Value::as_str) {
                            delta_content.push_str(t);
                        }
                    }
                }
                if !delta_content.is_empty() {
                    self.estimated_chars += delta_content.len() as i64;
                    self.completion_tokens += estimate_tokens(&delta_content);
                }
            }
        }
    }

    /// Flushes a trailing partial line (feed a newline at stream end).
    pub fn finish(&mut self) {
        if !self.leftover.is_empty() {
            self.feed(b"\n");
        }
    }
}

fn get_int_field(value: &Value, keys: &[&str]) -> i64 {
    for key in keys {
        if let Some(field) = value.get(*key) {
            if let Some(n) = field.as_i64() {
                return n;
            }
            if let Some(n) = field.as_f64() {
                return n as i64;
            }
        }
    }
    0
}

/// Extracts the requested model and stream flag from the request body in one
/// parse. `stream` must unambiguously be a JSON boolean.
pub fn request_model(body: &[u8]) -> Result<(String, bool), String> {
    let value: Value =
        serde_json::from_slice(body).map_err(|err| format!("invalid JSON request: {err}"))?;
    let Some(object) = value.as_object() else {
        return Err("invalid JSON request: expected an object".to_string());
    };
    let model = object
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if model.is_empty() {
        return Err("model is required".to_string());
    }
    let stream = parse_stream_flag(object.get("stream"))?;
    Ok((model, stream))
}

/// Accepts an absent, null, false or true `stream` value; any other type is
/// rejected with a clear message.
fn parse_stream_flag(raw: Option<&Value>) -> Result<bool, String> {
    match raw {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        Some(other) => Err(format!(
            "invalid `stream`: expected a JSON boolean (true or false), got {}",
            crate::api::truncate_echoed(&other.to_string())
        )),
    }
}

/// Whether a channel can serve the given protocol.
pub fn gateway_candidate_matcher(
    proto: GatewayProtocol,
    ch: &crate::store::Channel,
) -> bool {
    use crate::probe::{PROVIDER_ANTHROPIC, PROVIDER_OPENAI};
    match proto {
        GatewayProtocol::Anthropic => {
            ch.provider == PROVIDER_ANTHROPIC || ch.provider == PROVIDER_OPENAI
        }
        GatewayProtocol::Chat => {
            ch.provider == PROVIDER_OPENAI
                && ch.api_mode == crate::probe::API_MODE_CHAT_COMPLETIONS
        }
        GatewayProtocol::Responses => {
            ch.provider == PROVIDER_OPENAI && ch.api_mode == crate::probe::API_MODE_RESPONSES
        }
    }
}

/// Estimates prompt tokens from a request body when the upstream reports none.
pub fn estimate_prompt_tokens(body: &[u8]) -> i64 {
    if body.is_empty() {
        return 0;
    }
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        let chars = body.len() as i64;
        return if chars / 4 < 1 { 1 } else { chars / 4 };
    };
    let object = match value.as_object() {
        Some(object) => object,
        None => {
            let chars = body.len() as i64;
            return if chars / 4 < 1 { 1 } else { chars / 4 };
        }
    };

    let mut total_chars: i64 = 0;
    let count_any = |value: Option<&Value>| {
        let mut chars = 0i64;
        match value {
            Some(Value::String(text)) => chars += text.len() as i64,
            Some(Value::Array(items)) => {
                for item in items {
                    match item {
                        Value::String(text) => chars += text.len() as i64,
                        Value::Object(map) => {
                            if let Some(text) = map.get("text").and_then(Value::as_str) {
                                chars += text.len() as i64;
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        chars
    };
    total_chars += count_any(object.get("prompt"));
    total_chars += count_any(object.get("system"));
    if let Some(Value::Array(messages)) = object.get("messages") {
        for message in messages {
            total_chars += count_any(message.get("content"));
        }
    }
    if total_chars == 0 {
        total_chars = body.len() as i64;
    }
    let est = total_chars / 3;
    if est < 1 {
        1
    } else {
        est
    }
}

/// Extracts usage from a non-streaming response body.
pub fn extract_non_streaming_tokens(body: &[u8]) -> (i64, i64, i64) {
    if body.is_empty() {
        return (0, 0, 0);
    }
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return (0, 0, 0);
    };
    let Some(usage) = value.get("usage") else {
        return (0, 0, 0);
    };
    let mut prompt = get_int_field(usage, &["prompt_tokens"]);
    if prompt == 0 {
        prompt = get_int_field(usage, &["input_tokens"]);
    }
    let mut completion = get_int_field(usage, &["completion_tokens"]);
    if completion == 0 {
        completion = get_int_field(usage, &["output_tokens"]);
    }
    let mut total = get_int_field(usage, &["total_tokens"]);
    if total == 0 && (prompt > 0 || completion > 0) {
        total = prompt + completion;
    }
    (prompt, completion, total)
}

/// Connection-scoped headers a proxy must not forward (RFC 7230 §6.1), plus
/// `Content-Length`, which no longer describes the body once re-framed.
pub fn is_hop_by_hop_header(key: &str) -> bool {
    matches!(
        key.trim().to_lowercase().as_str(),
        "connection"
            | "proxy-connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "content-length"
    )
}

/// Forwards only protocol headers meaningful to an upstream provider.
/// Browser/session headers must never leak to a configured supplier.
pub fn is_forwardable_client_header(key: &str) -> bool {
    matches!(
        key.to_lowercase().as_str(),
        "accept"
            | "user-agent"
            | "anthropic-version"
            | "anthropic-beta"
            | "openai-organization"
            | "openai-project"
            | "openai-beta"
    )
}

/// Parses a `Retry-After` header (seconds or HTTP date).
pub fn parse_retry_after(value: &str) -> Duration {
    let value = value.trim();
    if value.is_empty() {
        return Duration::ZERO;
    }
    if let Ok(seconds) = value.parse::<u64>() {
        if seconds > 0 {
            return Duration::from_secs(seconds);
        }
    }
    if let Ok(when) = chrono::DateTime::parse_from_rfc2822(value) {
        let delay = when.with_timezone(&chrono::Utc) - chrono::Utc::now();
        if delay > chrono::Duration::zero() {
            return delay.to_std().unwrap_or(Duration::ZERO);
        }
    }
    Duration::ZERO
}

/// Maps an HTTP status to the closest Anthropic error type.
pub fn anthropic_error_type(status: i32) -> &'static str {
    match status {
        401 | 403 => "authentication_error",
        404 => "not_found_error",
        429 => "rate_limit_error",
        413 => "request_too_large",
        s if s >= 500 => "api_error",
        s if s >= 400 => "invalid_request_error",
        _ => "api_error",
    }
}

/// Whether a transport error message reads as the client having gone away.
pub fn is_client_closed_conn(message: &str) -> bool {
    let message = message.to_lowercase();
    message.contains("broken pipe")
        || message.contains("connection reset")
        || message.contains("client closed")
        || message.contains("context canceled")
}

#[cfg(test)]
mod tests {
    //! Ports the pure cases from `internal/api/gateway_test.go`.

    use super::*;
    use serde_json::json;

    #[test]
    fn request_model_requires_a_model() {
        assert_eq!(
            request_model(br#"{"model":"gpt-4o"}"#),
            Ok(("gpt-4o".to_string(), false))
        );
        assert!(request_model(br#"{"stream":true}"#).is_err());
        assert!(request_model(b"not json").is_err());
        assert_eq!(
            request_model(br#"{"model":"m","stream":true}"#),
            Ok(("m".to_string(), true))
        );
    }

    #[test]
    fn stream_flag_must_be_boolean() {
        assert_eq!(request_model(br#"{"model":"m","stream":null}"#), Ok(("m".to_string(), false)));
        assert_eq!(request_model(br#"{"model":"m","stream":false}"#), Ok(("m".to_string(), false)));
        // A stringly-typed value is rejected, not guessed at.
        let err = request_model(br#"{"model":"m","stream":"true"}"#).unwrap_err();
        assert!(err.contains("invalid `stream`"));
        assert!(request_model(br#"{"model":"m","stream":1}"#).is_err());
    }

    #[test]
    fn tracker_uses_exact_usage_and_marks_done() {
        let mut tracker = StreamTokenTracker::new(0);
        tracker.feed(
            br#"data: {"choices":[{"delta":{"content":"hello"}}]}
"#,
        );
        assert!(tracker.completion_tokens > 0);
        assert!(!tracker.has_exact_tokens);

        tracker.feed(
            br#"data: {"choices":[],"usage":{"prompt_tokens":7,"completion_tokens":11,"total_tokens":18}}
"#,
        );
        assert!(tracker.has_exact_tokens);
        assert_eq!(tracker.prompt_tokens, 7);
        assert_eq!(tracker.completion_tokens, 11);
        assert_eq!(tracker.total_tokens, 18);

        tracker.feed(b"data: [DONE]\n");
        assert!(tracker.saw_done);
    }

    #[test]
    fn tracker_handles_split_lines_across_chunks() {
        let mut tracker = StreamTokenTracker::new(0);
        tracker.feed(b"data: {\"choices\":[{\"finish_re");
        assert_eq!(tracker.finish_reason, "");
        tracker.feed(b"ason\":\"stop\"}]}\n");
        assert_eq!(tracker.finish_reason, "stop");
        assert!(tracker.saw_done);
    }

    #[test]
    fn tracker_recognises_anthropic_and_responses_markers() {
        let mut tracker = StreamTokenTracker::new(0);
        tracker.feed(b"data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":5}}}\n");
        assert_eq!(tracker.prompt_tokens, 5);
        tracker.feed(b"data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":9}}\n");
        assert_eq!(tracker.completion_tokens, 9);
        assert!(tracker.has_exact_tokens);

        let mut tracker = StreamTokenTracker::new(0);
        tracker.feed(b"data: {\"type\":\"response.completed\"}\n");
        assert!(tracker.saw_done);
    }

    #[test]
    fn tracker_records_in_band_stream_errors() {
        let mut tracker = StreamTokenTracker::new(0);
        tracker.feed(b"data: {\"error\":{\"message\":\"quota exceeded\"}}\n");
        assert_eq!(tracker.stream_error, "quota exceeded");
        assert_eq!(tracker.stream_error_status, 502);
    }

    #[test]
    fn prompt_token_estimation() {
        assert_eq!(estimate_prompt_tokens(b""), 0);
        let body = br#"{"messages":[{"content":"abcdefghijklmnopqrstuvwxyz"}]}"#;
        // A 26-char content string -> 26/3 = 8.
        assert_eq!(estimate_prompt_tokens(body), 8);
        assert_eq!(estimate_prompt_tokens(b"12345"), 1);
    }

    #[test]
    fn non_streaming_token_extraction() {
        let body = br#"{"usage":{"prompt_tokens":3,"completion_tokens":4}}"#;
        assert_eq!(extract_non_streaming_tokens(body), (3, 4, 7));
        let anthropic = br#"{"usage":{"input_tokens":5,"output_tokens":6}}"#;
        assert_eq!(extract_non_streaming_tokens(anthropic), (5, 6, 11));
        assert_eq!(extract_non_streaming_tokens(br#"{}"#), (0, 0, 0));
    }

    #[test]
    fn header_policy() {
        assert!(is_hop_by_hop_header("Content-Length"));
        assert!(is_hop_by_hop_header("connection"));
        assert!(!is_hop_by_hop_header("content-type"));

        assert!(is_forwardable_client_header("Anthropic-Beta"));
        assert!(is_forwardable_client_header("User-Agent"));
        // Cookies, origin and referrer must never leak upstream.
        assert!(!is_forwardable_client_header("cookie"));
        assert!(!is_forwardable_client_header("origin"));
        assert!(!is_forwardable_client_header("referer"));
    }

    #[test]
    fn retry_after_and_error_type() {
        assert_eq!(parse_retry_after("120"), Duration::from_secs(120));
        assert_eq!(parse_retry_after(""), Duration::ZERO);
        assert_eq!(parse_retry_after("garbage"), Duration::ZERO);

        assert_eq!(anthropic_error_type(401), "authentication_error");
        assert_eq!(anthropic_error_type(429), "rate_limit_error");
        assert_eq!(anthropic_error_type(500), "api_error");
        assert_eq!(anthropic_error_type(400), "invalid_request_error");
    }

    #[test]
    fn candidate_matcher_by_protocol() {
        let mut ch = crate::store::Channel {
            provider: "openai".to_string(),
            api_mode: "chat_completions".to_string(),
            ..Default::default()
        };
        assert!(gateway_candidate_matcher(GatewayProtocol::Chat, &ch));
        assert!(!gateway_candidate_matcher(GatewayProtocol::Responses, &ch));
        assert!(gateway_candidate_matcher(GatewayProtocol::Anthropic, &ch));

        ch.provider = "anthropic".to_string();
        assert!(gateway_candidate_matcher(GatewayProtocol::Anthropic, &ch));
        assert!(!gateway_candidate_matcher(GatewayProtocol::Chat, &ch));
    }

    #[test]
    fn json_helper_smoke() {
        // Guards the json! import used across tests.
        assert_eq!(json!({"a": 1})["a"], 1);
    }
}
