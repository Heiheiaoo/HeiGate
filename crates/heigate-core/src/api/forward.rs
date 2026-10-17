//! Gateway forwarding primitives that carry no HTTP plumbing: the streaming
//! token tracker, request parsing, token estimation, and header policy.
//! Ported from `internal/api/gateway.go`.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::Value;

use crate::protocol::estimate_tokens;
use crate::protocol::stream::SseEventDecoder;

/// Measures semantic output rather than SSE headers, keepalives or usage.
#[derive(Debug, Default)]
pub struct StreamTtftTracker {
    decoder: SseEventDecoder,
    leftover: Vec<u8>,
    text_ms: Option<i64>,
    other_ms: Option<i64>,
    last_chunk_ms: i64,
}

#[cfg(test)]
mod ttft_tests {
    use super::{StreamTokenTracker, StreamTtftTracker};
    use crate::protocol::stream::MAX_SSE_LINE_BYTES;

    #[test]
    fn trackers_bound_unterminated_and_terminated_sse_lines() {
        for terminated in [false, true] {
            let mut chunk = vec![b'x'; MAX_SSE_LINE_BYTES + 1];
            if terminated {
                chunk.push(b'\n');
            }
            let mut tracker = StreamTokenTracker::default();
            let mut ttft = StreamTtftTracker::default();
            for part in chunk.chunks(64 << 10) {
                tracker.feed(part);
                ttft.feed(part, 1);
                assert!(tracker.leftover.len() <= MAX_SSE_LINE_BYTES);
                assert!(ttft.leftover.len() <= MAX_SSE_LINE_BYTES);
            }
            assert!(tracker.buffer_exceeded);
            assert_eq!(tracker.stream_error_status, 502);
            tracker.feed(b"data: [DONE]\n\n");
            assert!(!tracker.saw_done);
        }
    }

    #[test]
    fn records_visible_text_before_reasoning_or_tools() {
        let mut tracker = StreamTtftTracker::default();
        tracker.feed(
            b"data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"think\"}}]}\n\n",
            40,
        );
        assert_eq!(tracker.ttft_ms(), Some(40));
        tracker.feed(
            b"data: {\"choices\":[{\"delta\":{\"content\":\"answer\"}}]}\n\n",
            90,
        );
        assert_eq!(tracker.ttft_ms(), Some(90));
    }

    #[test]
    fn handles_split_sse_lines_and_responses_text() {
        let mut tracker = StreamTtftTracker::default();
        tracker.feed(b"data: {\"type\":\"response.output_text.", 1);
        tracker.feed(b"delta\",\"delta\":\"hello\"}\n\n", 27);
        assert_eq!(tracker.ttft_ms(), Some(27));
    }

    #[test]
    fn ignores_done_and_empty_deltas() {
        let mut tracker = StreamTtftTracker::default();
        tracker.feed(
            b"data: {\"choices\":[{\"delta\":{}}]}\n\ndata: [DONE]\n\n",
            20,
        );
        assert_eq!(tracker.ttft_ms(), None);
    }

    #[test]
    fn supports_tool_only_anthropic_and_trailing_unterminated_output() {
        let mut tracker = StreamTtftTracker::default();
        tracker.feed(b"data: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"tool_use\",\"name\":\"search\"}}\n\n", 70);
        assert_eq!(tracker.ttft_ms(), Some(70));
        let mut tracker = StreamTtftTracker::default();
        tracker.feed(
            b"data: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\"hello\"}}",
            42,
        );
        tracker.finish();
        assert_eq!(tracker.ttft_ms(), Some(42));
    }
}

impl StreamTtftTracker {
    pub fn feed(&mut self, chunk: &[u8], elapsed_ms: i64) {
        if self.text_ms.is_some() {
            return;
        }
        self.last_chunk_ms = elapsed_ms;
        for event in self.decoder.feed(chunk) {
            let Ok(value) = serde_json::from_slice::<Value>(&event) else {
                continue;
            };
            let (text, other) = semantic_stream_output(&value);
            if text && self.text_ms.is_none() {
                self.text_ms = Some(elapsed_ms.max(1));
            }
            if other && self.other_ms.is_none() {
                self.other_ms = Some(elapsed_ms.max(1));
            }
        }
        self.leftover.clear();
        self.leftover.extend_from_slice(self.decoder.pending_line());
    }

    pub fn ttft_ms(&self) -> Option<i64> {
        self.text_ms.or(self.other_ms)
    }

    pub fn finish(&mut self) {
        for event in self.decoder.finish() {
            let Ok(value) = serde_json::from_slice::<Value>(&event) else {
                continue;
            };
            let (text, other) = semantic_stream_output(&value);
            if text && self.text_ms.is_none() {
                self.text_ms = Some(self.last_chunk_ms.max(1));
            }
            if other && self.other_ms.is_none() {
                self.other_ms = Some(self.last_chunk_ms.max(1));
            }
        }
        self.leftover.clear();
    }
}

/// Shared by prefix failover and TTFT: metadata alone is not generated output.
pub(crate) fn semantic_stream_output(value: &Value) -> (bool, bool) {
    let event = value.get("type").and_then(Value::as_str).unwrap_or("");
    let nonempty = |v: Option<&Value>| v.and_then(Value::as_str).is_some_and(|s| !s.is_empty());
    let deltas = value.get("choices").and_then(Value::as_array);
    let text = deltas.is_some_and(|choices| {
        choices
            .iter()
            .any(|c| c.get("delta").is_some_and(|d| nonempty(d.get("content"))))
    }) || (event == "content_block_delta"
        && value.get("delta").is_some_and(|d| nonempty(d.get("text"))))
        || (event == "content_block_start"
            && value
                .get("content_block")
                .is_some_and(|d| nonempty(d.get("text"))))
        || (event == "response.output_text.delta" && nonempty(value.get("delta")));
    let other = deltas.is_some_and(|choices| {
        choices.iter().any(|c| {
            c.get("delta").is_some_and(|d| {
                nonempty(d.get("reasoning_content"))
                    || nonempty(d.get("reasoning"))
                    || d.get("audio")
                        .is_some_and(|a| nonempty(a.get("data")) || nonempty(a.get("transcript")))
                    || d.get("tool_calls")
                        .and_then(Value::as_array)
                        .is_some_and(|calls| {
                            calls.iter().any(|c| {
                                c.get("function").is_some_and(|f| {
                                    nonempty(f.get("name")) || nonempty(f.get("arguments"))
                                })
                            })
                        })
                    || d.get("function_call")
                        .is_some_and(|f| nonempty(f.get("name")) || nonempty(f.get("arguments")))
            })
        })
    }) || (event == "content_block_delta"
        && value
            .get("delta")
            .is_some_and(|d| nonempty(d.get("thinking")) || nonempty(d.get("partial_json"))))
        || (event == "content_block_start"
            && value.get("content_block").is_some_and(|d| {
                matches!(
                    d.get("type").and_then(Value::as_str),
                    Some("tool_use" | "server_tool_use")
                ) && nonempty(d.get("name"))
            }))
        || (matches!(
            event,
            "response.reasoning_text.delta"
                | "response.reasoning_summary_text.delta"
                | "response.function_call_arguments.delta"
                | "response.audio.delta"
                | "response.output_audio.delta"
        ) && nonempty(value.get("delta")))
        || (event == "response.output_item.added"
            && value.get("item").is_some_and(|i| {
                i.get("type").and_then(Value::as_str) == Some("function_call")
                    && nonempty(i.get("name"))
            }));
    (text, other)
}

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
    pub has_exact_prompt_tokens: bool,
    pub translated_from_chat: bool,
    pub estimated_chars: i64,
    pub leftover: Vec<u8>,
    decoder: SseEventDecoder,
    pub saw_output: bool,
    pub saw_done: bool,
    pub finish_reason: String,
    pub stream_error: String,
    pub stream_error_status: i32,
    pub expected_choices: usize,
    pub(crate) buffer_exceeded: bool,
    choices: BTreeMap<i64, bool>,
    global_done: bool,
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
        if chunk.is_empty() || self.buffer_exceeded {
            return;
        }
        for event in self.decoder.feed(chunk) {
            if self.process_event(&event) {
                return;
            }
        }
        self.leftover.clear();
        self.leftover.extend_from_slice(self.decoder.pending_line());
        if self.decoder.exceeded {
            self.leftover.clear();
            self.buffer_exceeded = true;
            self.stream_error = "upstream SSE line or event exceeded size limit".into();
            self.stream_error_status = 502;
        }
    }

    fn process_event(&mut self, event: &[u8]) -> bool {
        let payload = String::from_utf8_lossy(event);
        if payload == "[DONE]" {
            self.global_done = true;
            self.saw_done = true;
            return false;
        }
        if payload.is_empty() {
            return false;
        }

        let Ok(generic) = serde_json::from_str::<Value>(&payload) else {
            return false;
        };
        let (text, other) = semantic_stream_output(&generic);
        self.saw_output |= text || other;
        if let Some((message, status)) = crate::api::upstream_error_payload(&payload) {
            self.stream_error = message;
            self.stream_error_status = status;
            return false;
        }

        if let Some(choices) = generic.get("choices").and_then(Value::as_array) {
            for (position, choice) in choices.iter().enumerate() {
                let index = choice
                    .get("index")
                    .and_then(Value::as_i64)
                    .unwrap_or(position as i64);
                if self.choices.len() >= 1024 && !self.choices.contains_key(&index) {
                    self.buffer_exceeded = true;
                    self.stream_error = "upstream choice count exceeded size limit".into();
                    self.stream_error_status = 502;
                    return true;
                }
                let finished = self.choices.entry(index).or_default();
                if let Some(finish) = choice
                    .get("finish_reason")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                {
                    *finished = true;
                    if self.finish_reason.is_empty() || index == 0 {
                        self.finish_reason = finish.to_string();
                    }
                }
            }
            if !choices.is_empty() && !self.global_done {
                self.saw_done = self.choices.len() >= self.expected_choices.max(1)
                    && self.choices.values().all(|finished| *finished);
            }
        }

        let event_type = generic.get("type").and_then(Value::as_str).unwrap_or("");
        // Anthropic message_stop, and the Responses API's response.completed,
        // are authoritative completion markers.
        if event_type == "message_stop" || event_type == "response.completed" {
            self.global_done = true;
            self.saw_done = true;
            if self.finish_reason.is_empty() {
                self.finish_reason = "stop".to_string();
            }
        }
        if event_type == "response.incomplete" {
            self.global_done = true;
            self.saw_done = true;
            self.finish_reason = incomplete_finish_reason(generic.get("response")).to_string();
        }
        if event_type == "message_delta" {
            if let Some(reason) = generic
                .get("delta")
                .and_then(|d| d.get("stop_reason"))
                .and_then(Value::as_str)
            {
                self.finish_reason = reason.to_string();
            }
        }

        // Responses nests the final usage inside `response`; Anthropic's
        // message_start only supplies input usage, so keep estimating output.
        let usage = if matches!(event_type, "response.completed" | "response.incomplete") {
            generic.get("response").and_then(|r| r.get("usage"))
        } else if event_type == "message_start" {
            generic.get("message").and_then(|m| m.get("usage"))
        } else {
            generic.get("usage")
        };
        if let Some(usage) = usage.filter(|u| u.is_object()) {
            if let Some(p) = optional_int_field(usage, &["prompt_tokens", "input_tokens"]) {
                // Only translated Chat streams use zero as an initial
                // placeholder; native Anthropic usage is authoritative.
                if p > 0 || event_type != "message_start" || !self.translated_from_chat {
                    self.prompt_tokens = p;
                    self.has_exact_prompt_tokens = true;
                }
            }
            if event_type != "message_start" {
                if let Some(c) = optional_int_field(usage, &["completion_tokens", "output_tokens"])
                {
                    self.completion_tokens = c;
                    self.has_exact_tokens = true;
                }
            }
            self.total_tokens = optional_int_field(usage, &["total_tokens"])
                .unwrap_or(self.prompt_tokens + self.completion_tokens);
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
            if matches!(
                event_type,
                "response.output_text.delta"
                    | "response.reasoning_text.delta"
                    | "response.reasoning_summary_text.delta"
                    | "response.function_call_arguments.delta"
            ) {
                if let Some(delta) = generic.get("delta").and_then(Value::as_str) {
                    delta_content.push_str(delta);
                }
            }
            if !delta_content.is_empty() {
                self.estimated_chars += delta_content.len() as i64;
                self.completion_tokens += estimate_tokens(&delta_content);
                self.total_tokens = self.prompt_tokens + self.completion_tokens;
            }
        }
        false
    }

    /// Flushes a trailing data event at stream end, while still requiring an
    /// authoritative completion marker to count the request as successful.
    pub fn finish(&mut self) {
        for event in self.decoder.finish() {
            if self.process_event(&event) {
                continue;
            }
        }
        self.leftover.clear();
        self.leftover.extend_from_slice(self.decoder.pending_line());
        if self.decoder.exceeded {
            self.leftover.clear();
            self.buffer_exceeded = true;
            self.stream_error = "upstream SSE line or event exceeded size limit".into();
            self.stream_error_status = 502;
        }
    }

    pub(crate) fn has_authoritative_end(&self) -> bool {
        self.global_done
    }
}

fn incomplete_finish_reason(response: Option<&Value>) -> &str {
    match response
        .and_then(|r| r.get("incomplete_details"))
        .and_then(|details| details.get("reason"))
        .and_then(Value::as_str)
        .unwrap_or("incomplete")
    {
        "max_output_tokens" => "length",
        reason => reason,
    }
}

pub(crate) fn non_streaming_finish_reason(body: &[u8]) -> String {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return "stop".to_string();
    };
    let reason = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("finish_reason"))
        .or_else(|| value.get("stop_reason"))
        .and_then(Value::as_str)
        .filter(|reason| !reason.is_empty());
    reason
        .unwrap_or_else(|| {
            if value.get("status").and_then(Value::as_str) == Some("incomplete") {
                incomplete_finish_reason(Some(&value))
            } else {
                "stop"
            }
        })
        .to_string()
}

fn optional_int_field(value: &Value, keys: &[&str]) -> Option<i64> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|n| n as i64)))
    })
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
pub fn gateway_candidate_matcher(proto: GatewayProtocol, ch: &crate::store::Channel) -> bool {
    use crate::probe::{PROVIDER_ANTHROPIC, PROVIDER_OPENAI};
    match proto {
        GatewayProtocol::Anthropic => {
            ch.provider == PROVIDER_ANTHROPIC
                || (ch.provider == PROVIDER_OPENAI
                    && ch.api_mode == crate::probe::API_MODE_CHAT_COMPLETIONS)
        }
        GatewayProtocol::Chat => {
            ch.provider == PROVIDER_OPENAI && ch.api_mode == crate::probe::API_MODE_CHAT_COMPLETIONS
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
    exact_non_streaming_tokens(body).unwrap_or_default()
}

pub(crate) fn exact_non_streaming_tokens(body: &[u8]) -> Option<(i64, i64, i64)> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let usage = value.get("usage")?;
    let prompt = optional_int_field(usage, &["prompt_tokens", "input_tokens"]);
    let completion = optional_int_field(usage, &["completion_tokens", "output_tokens"]);
    let total = optional_int_field(usage, &["total_tokens"]);
    if prompt.is_none() && completion.is_none() && total.is_none() {
        return None;
    }
    let (prompt, completion) = (prompt.unwrap_or(0), completion.unwrap_or(0));
    Some((prompt, completion, total.unwrap_or(prompt + completion)))
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
        assert_eq!(
            request_model(br#"{"model":"m","stream":null}"#),
            Ok(("m".to_string(), false))
        );
        assert_eq!(
            request_model(br#"{"model":"m","stream":false}"#),
            Ok(("m".to_string(), false))
        );
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
        tracker.feed(
            b"data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":5}}}\n",
        );
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

    #[test]
    fn responses_usage_overrides_estimates_including_zero_and_split_unicode() {
        let mut tracker = StreamTokenTracker::new(7);
        let delta = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"你好\"}\n\n";
        for byte in delta.bytes() {
            tracker.feed(&[byte]);
        }
        assert_eq!(tracker.completion_tokens, 2);
        assert_eq!(tracker.total_tokens, 9);
        tracker.feed(b"data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":100,\"output_tokens\":20,\"total_tokens\":120}}}\n\n");
        assert_eq!(
            (
                tracker.prompt_tokens,
                tracker.completion_tokens,
                tracker.total_tokens
            ),
            (100, 20, 120)
        );
        assert!(tracker.has_exact_tokens && tracker.saw_done);
        let mut zero = StreamTokenTracker::new(7);
        zero.feed(delta.as_bytes());
        zero.feed(b"data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":0,\"output_tokens\":0,\"total_tokens\":0}}}\n\n");
        assert_eq!(
            (
                zero.prompt_tokens,
                zero.completion_tokens,
                zero.total_tokens
            ),
            (0, 0, 0)
        );
        assert!(zero.has_exact_tokens);
    }

    #[test]
    fn native_zero_input_is_exact_but_translated_placeholder_is_not() {
        for translated in [false, true] {
            let mut tracker = StreamTokenTracker::new(30);
            tracker.translated_from_chat = translated;
            tracker.feed(b"data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":0,\"output_tokens\":1}}}\n\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":20}}\n\ndata: {\"type\":\"message_stop\"}\n\n");
            let input = if translated { 30 } else { 0 };
            assert_eq!(
                (
                    tracker.prompt_tokens,
                    tracker.completion_tokens,
                    tracker.total_tokens
                ),
                (input, 20, input + 20)
            );
            assert_eq!(tracker.has_exact_prompt_tokens, !translated);
        }
    }

    #[test]
    fn empty_usage_does_not_disable_stream_estimation() {
        let mut tracker = StreamTokenTracker::new(1);
        tracker.feed(b"data: {\"usage\":{}}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n");
        assert!(!tracker.has_exact_tokens);
        assert!(tracker.completion_tokens > 0);
    }
}
