//! Gateway request/response policy: model aliasing, auto-model ranking, and
//! the upstream error-detection rules. Ported from the Go `internal/api/gateway.go`
//! policy helpers.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::probe::{STATUS_DEGRADED, STATUS_ERROR, STATUS_FAILED, STATUS_OPERATIONAL};
use crate::resilience::CIRCUIT_OPEN;
use crate::store::Channel;

/// The sentinel model name that triggers automatic best-model routing.
pub const AUTO_MODEL_NAME: &str = "Auto";

/// Bounds how much of an untrusted value is echoed back in an error message.
const MAX_ECHOED_ERROR_TEXT: usize = 200;

/// Whether the channel advertises `model` as its primary or an extra model.
pub fn channel_supports_model(ch: &Channel, model: &str) -> bool {
    if ch.primary_model.eq_ignore_ascii_case(model) {
        return true;
    }
    ch.extra_models
        .iter()
        .any(|m| m.eq_ignore_ascii_case(model))
}

/// Whether a channel is a sensible target right now: enabled, not benched by an
/// open circuit or active cooldown, and not left in a failed state by its last
/// check. A never-probed channel counts as usable.
pub fn channel_serving_traffic(ch: &Channel) -> bool {
    if ch.circuit_state == CIRCUIT_OPEN {
        return false;
    }
    if let Some(until) = ch.cooldown_until {
        if chrono::Utc::now() < until {
            return false;
        }
    }
    matches!(
        ch.last_status.as_str(),
        STATUS_OPERATIONAL | STATUS_DEGRADED | "healthy" | "" | "unknown"
    )
}

/// The best configured upstream model for one already-eligible channel.
/// Model-level probe state wins over the channel headline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoModelChoice {
    pub name: String,
    pub rank: i64,
    pub lat: i64,
    pub order: usize,
}

const RANK_UNKNOWN: i64 = 99;

/// Selects the best configured model for a channel.
pub fn best_auto_choice(ch: Option<&Channel>) -> AutoModelChoice {
    let Some(ch) = ch else {
        return AutoModelChoice {
            name: String::new(),
            rank: RANK_UNKNOWN,
            lat: i64::MAX,
            order: 0,
        };
    };

    let mut models = vec![ch.primary_model.clone()];
    models.extend(ch.extra_models.clone());

    let mut best = AutoModelChoice {
        name: String::new(),
        rank: RANK_UNKNOWN,
        lat: i64::MAX,
        order: models.len() + 1,
    };

    for (index, name) in models.iter().enumerate() {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let mut current = AutoModelChoice {
            name: name.to_string(),
            rank: 2,
            lat: i64::MAX,
            order: index,
        };

        if name.eq_ignore_ascii_case(&ch.primary_model) {
            match ch.last_status.as_str() {
                STATUS_OPERATIONAL | "healthy" => current.rank = 0,
                STATUS_DEGRADED => current.rank = 1,
                STATUS_ERROR | STATUS_FAILED => current.rank = 3,
                _ => {}
            }
            if let Some(ttft) = ch.last_ttft_ms.filter(|v| *v > 0) {
                current.lat = ttft;
            } else if let Some(latency) = ch.last_latency_ms.filter(|v| *v > 0) {
                current.lat = latency;
            }
        }

        for health in &ch.model_health {
            if !health.model.trim().eq_ignore_ascii_case(name) {
                continue;
            }
            current.rank = match health.status.as_str() {
                STATUS_OPERATIONAL | "healthy" => 0,
                STATUS_DEGRADED => 1,
                STATUS_ERROR | STATUS_FAILED => 3,
                _ => 2,
            };
            if let Some(ttft) = health.ttft_ms.filter(|v| *v > 0) {
                current.lat = ttft;
            } else if let Some(latency) = health.latency_ms.filter(|v| *v > 0) {
                current.lat = latency;
            }
            break;
        }

        let better = current.rank < best.rank
            || (current.rank == best.rank && current.lat < best.lat)
            || (current.rank == best.rank && current.lat == best.lat && current.order < best.order);
        if better {
            best = current;
        }
    }
    best
}

/// The best configured upstream model name for a channel.
pub fn best_auto_model(ch: Option<&Channel>) -> String {
    best_auto_choice(ch).name
}

/// One attempt in the routing and failover process.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TraceStep {
    pub index: i64,
    pub channel_name: String,
    pub channel_id: i64,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub endpoint: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub model: String,
    pub attempt: i64,
    #[serde(skip_serializing_if = "is_zero", default)]
    pub status_code: i64,
    #[serde(skip_serializing_if = "is_zero", default)]
    pub latency_ms: i64,
    #[serde(skip_serializing_if = "is_zero", default)]
    pub ttft_ms: i64,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub error: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub raw_error: String,
    pub succeeded: bool,
}

fn is_zero(value: &i64) -> bool {
    *value == 0
}

/// Caps a value before it is placed in an error message, cutting on a char
/// boundary so the result is never invalid UTF-8.
pub fn truncate_echoed(text: &str) -> String {
    if text.len() <= MAX_ECHOED_ERROR_TEXT {
        return text.to_string();
    }
    let mut cut = MAX_ECHOED_ERROR_TEXT;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}...", &text[..cut])
}

/// Extracts a human-readable message from an upstream error body.
pub fn parse_upstream_error_message(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return String::new();
    }
    if let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(raw) {
        if let Some(Value::Object(error)) = obj.get("error") {
            if let Some(message) = error.get("message").and_then(Value::as_str) {
                if !message.trim().is_empty() {
                    return message.trim().to_string();
                }
            }
        }
        if let Some(text) = obj.get("error").and_then(Value::as_str) {
            if !text.trim().is_empty() {
                return text.trim().to_string();
            }
        }
        if let Some(message) = obj.get("message").and_then(Value::as_str) {
            if !message.trim().is_empty() {
                return message.trim().to_string();
            }
        }
        if let Some(message) = obj.get("msg").and_then(Value::as_str) {
            if !message.trim().is_empty() {
                return message.trim().to_string();
            }
        }
    }
    truncate_echoed(raw)
}

/// Rewrites an unsupported `reasoning_effort` only when the upstream's message
/// names what it accepts. Returns the adapted body, or `None` when nothing
/// changed.
pub fn adapt_unsupported_reasoning_effort(body: &[u8], message: &str) -> Option<Vec<u8>> {
    let supported = supported_reasoning_efforts(message);
    if supported.is_empty() {
        return None;
    }
    let mut payload: Value = serde_json::from_slice(body).ok()?;
    let object = payload.as_object_mut()?;

    let mut changed = false;
    if let Some(effort) = object.get("reasoning_effort").and_then(Value::as_str) {
        if let Some(replacement) = compatible_reasoning_effort(effort, &supported) {
            if replacement != effort {
                object.insert("reasoning_effort".to_string(), Value::String(replacement));
                changed = true;
            }
        }
    }
    if let Some(Value::Object(reasoning)) = object.get_mut("reasoning") {
        if let Some(effort) = reasoning.get("effort").and_then(Value::as_str) {
            if let Some(replacement) = compatible_reasoning_effort(effort, &supported) {
                if replacement != effort {
                    reasoning.insert("effort".to_string(), Value::String(replacement));
                    changed = true;
                }
            }
        }
    }

    if !changed {
        return None;
    }
    serde_json::to_vec(&payload).ok()
}

fn supported_reasoning_efforts(message: &str) -> Vec<String> {
    let lower = message.to_lowercase();
    let marker = lower
        .find("supported types")
        .or_else(|| lower.find("supported values"));
    let Some(marker) = marker else {
        return Vec::new();
    };
    let text = &lower[marker..];
    let known = ["xhigh", "high", "medium", "low", "minimal"];
    let mut seen: Vec<&str> = Vec::new();
    for field in text.split_whitespace() {
        let field = field.trim_matches(|c: char| ".,;:()[]{}\"'".contains(c));
        if let Some(candidate) = known.iter().find(|k| **k == field) {
            if !seen.contains(candidate) {
                seen.push(candidate);
            }
        }
    }
    known
        .iter()
        .filter(|k| seen.contains(k))
        .map(|k| k.to_string())
        .collect()
}

fn compatible_reasoning_effort(requested: &str, supported: &[String]) -> Option<String> {
    let requested = requested.trim().to_lowercase();
    if supported.iter().any(|e| *e == requested) {
        return Some(requested);
    }
    let preferences: &[(&str, &[&str])] = &[
        ("high", &["xhigh", "medium", "low"]),
        ("xhigh", &["medium", "high", "low"]),
        ("medium", &["high", "xhigh", "low"]),
        ("low", &["medium", "high", "xhigh"]),
        ("minimal", &["low", "medium", "high", "xhigh"]),
    ];
    let candidates = preferences
        .iter()
        .find(|(key, _)| *key == requested)
        .map(|(_, list)| *list)?;
    for candidate in candidates {
        if supported.iter().any(|e| e == candidate) {
            return Some((*candidate).to_string());
        }
    }
    None
}

/// Reports whether an upstream response body is an error and extracts its
/// message and an HTTP-equivalent status.
///
/// Policy — a body counts as an error only when it carries an *unambiguous*
/// failure signal, because a false positive fails over a healthy channel (and
/// benches it) while a false negative hands the client an empty answer. The two
/// signals are (1) a non-empty `error` value and (2) a top-level `code` that is
/// a 4xx/5xx. Both are also checked one level down under `response` (the
/// Responses API reports `response.failed` there). A bare top-level `message`
/// is deliberately NOT a signal.
pub fn upstream_error_payload(raw: &str) -> Option<(String, i32)> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let envelope = value.as_object()?;
    if let Some(found) = classify_error_envelope(envelope, raw) {
        return Some(found);
    }
    if let Some(Value::Object(inner)) = envelope.get("response") {
        let nested_raw = envelope.get("response").map(Value::to_string).unwrap_or_default();
        if let Some(found) = classify_error_envelope(inner, &nested_raw) {
            return Some(found);
        }
    }
    None
}

fn classify_error_envelope(envelope: &Map<String, Value>, raw: &str) -> Option<(String, i32)> {
    let has_error_slot = envelope.contains_key("error");
    let error_slot_empty = !has_error_slot
        || is_empty_error_value(envelope.get("error"));
    let status_from_body = failing_status_code(envelope);
    if error_slot_empty && status_from_body == 0 {
        return None;
    }

    let message = parse_upstream_error_message(raw);

    let (detail_type, detail_code, detail_message) = match envelope.get("error") {
        Some(Value::Object(error)) => (
            error.get("type").and_then(Value::as_str).unwrap_or("").to_string(),
            error.get("code").cloned(),
            error.get("message").and_then(Value::as_str).unwrap_or("").to_string(),
        ),
        Some(Value::String(text)) => (String::new(), None, text.clone()),
        _ => (String::new(), None, String::new()),
    };

    // Status precedence: a status named by the body beats one guessed from text.
    let status = if status_from_body != 0 {
        status_from_body
    } else if let Some(code) = detail_code.as_ref().and_then(decode_status_code) {
        if (400..=599).contains(&code) {
            code
        } else {
            classification_status(&detail_type, &detail_message, "")
        }
    } else {
        let code_text = detail_code
            .as_ref()
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        classification_status(&detail_type, &detail_message, &code_text)
    };

    Some((message, status))
}

fn classification_status(detail_type: &str, message: &str, code_text: &str) -> i32 {
    let classification = format!("{detail_type} {code_text} {message}").to_lowercase();
    let has = |needle: &str| classification.contains(needle);
    if has("rate_limit") || has("rate limit") || has("too_many_requests") || has("too many requests")
    {
        429
    } else if has("authentication") || has("unauthorized") || has("invalid_api_key") {
        401
    } else if has("permission") || has("forbidden") {
        403
    } else if has("model_not_found") || has("unknown model") {
        404
    } else {
        502
    }
}

/// The HTTP-equivalent status named by a top-level `code`, or 0. Only 4xx/5xx
/// count.
fn failing_status_code(envelope: &Map<String, Value>) -> i32 {
    let Some(raw) = envelope.get("code") else {
        return 0;
    };
    match decode_status_code(raw) {
        Some(code) if (400..=599).contains(&code) => code,
        _ => 0,
    }
}

/// Reads a status from a `code` that may be a JSON number or numeric string.
fn decode_status_code(raw: &Value) -> Option<i32> {
    match raw {
        Value::Number(number) => number
            .as_i64()
            .map(|n| n as i32)
            .or_else(|| number.as_f64().filter(|f| *f == f.trunc()).map(|f| f as i32)),
        Value::String(text) => text.trim().parse::<i32>().ok(),
        _ => None,
    }
}

fn is_empty_error_value(value: Option<&Value>) -> bool {
    match value {
        None => true,
        Some(value) => is_empty_error_detail(value),
    }
}

/// Whether a decoded error value carries no information: null, a falsy scalar,
/// an empty container, or a container whose values are all themselves empty.
fn is_empty_error_detail(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Bool(b) => !*b,
        Value::Number(n) => n.as_f64().map(|f| f == 0.0).unwrap_or(false),
        Value::String(s) => s.trim().is_empty(),
        Value::Object(map) => map.values().all(is_empty_error_detail),
        Value::Array(items) => items.iter().all(is_empty_error_detail),
    }
}

/// Detects an error frame inside an SSE prefix, returning its message and
/// status.
pub fn upstream_stream_error(prefix: &str) -> Option<(String, i32)> {
    for line in prefix.split('\n') {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("data:") else {
            continue;
        };
        let payload = rest.trim();
        if payload.is_empty() || payload == "[DONE]" {
            continue;
        }
        if let Some(found) = upstream_error_payload(payload) {
            return Some(found);
        }
    }
    None
}

/// Whether an SSE prefix already carries model output.
pub fn upstream_stream_has_output(prefix: &str) -> bool {
    for line in prefix.split('\n') {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("data:") else {
            continue;
        };
        let payload = rest.trim();
        if payload.is_empty() || payload == "[DONE]" {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(payload) else {
            continue;
        };
        match value.get("type").and_then(Value::as_str) {
            Some("response.output_text.delta")
            | Some("response.reasoning_text.delta")
            | Some("response.reasoning_summary_text.delta")
            | Some("response.function_call_arguments.delta")
            | Some("response.audio.delta")
            | Some("content_block_delta") => return true,
            _ => {}
        }
        if let Some(choices) = value.get("choices").and_then(Value::as_array) {
            if let Some(choice) = choices.first() {
                if let Some(delta) = choice.get("delta").and_then(Value::as_object) {
                    if !delta.is_empty() {
                        return true;
                    }
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    //! Locks the upstream-error policy shape matrix (Go's
    //! `TestUpstreamErrorPayloadPolicy`).

    use super::*;

    fn is_error(body: &str) -> bool {
        upstream_error_payload(body).is_some()
    }

    #[test]
    fn empty_error_slots_are_not_errors() {
        for body in [
            "{}",
            r#"{"error":null}"#,
            r#"{"error":false}"#,
            r#"{"error":0}"#,
            r#"{"error":""}"#,
            r#"{"error":{}}"#,
            r#"{"error":[]}"#,
            r#"{"error":{"type":"","message":""}}"#,
            r#"{"message":"served by node-3"}"#,
            r#"{"code":0}"#,
            r#"{"code":200}"#,
            r#"{"code":302}"#,
            r#"{"choices":[]}"#,
        ] {
            assert!(!is_error(body), "should not be an error: {body}");
        }
    }

    #[test]
    fn unambiguous_signals_are_errors() {
        assert!(is_error(r#"{"error":{"message":"bad key"}}"#));
        assert!(is_error(r#"{"error":"boom"}"#));
        assert!(is_error(r#"{"code":500}"#));
        assert!(is_error(r#"{"code":429}"#));
        assert!(is_error(r#"{"code":"503"}"#));
        assert!(is_error(r#"{"response":{"error":{"message":"failed"}}}"#));
    }

    #[test]
    fn status_precedence_and_classification() {
        // A body-named code wins.
        assert_eq!(upstream_error_payload(r#"{"code":503}"#).unwrap().1, 503);
        // Nested error.code is authoritative.
        assert_eq!(
            upstream_error_payload(r#"{"error":{"code":401,"message":"x"}}"#)
                .unwrap()
                .1,
            401
        );
        // Text classification when no code is present.
        assert_eq!(
            upstream_error_payload(r#"{"error":{"message":"rate limit exceeded"}}"#)
                .unwrap()
                .1,
            429
        );
        assert_eq!(
            upstream_error_payload(r#"{"error":{"type":"authentication_error","message":"no"}}"#)
                .unwrap()
                .1,
            401
        );
        // Default when nothing identifies the class.
        assert_eq!(
            upstream_error_payload(r#"{"error":{"message":"weird"}}"#).unwrap().1,
            502
        );
    }

    #[test]
    fn message_extraction() {
        assert_eq!(
            parse_upstream_error_message(r#"{"error":{"message":"  hi  "}}"#),
            "hi"
        );
        assert_eq!(parse_upstream_error_message(r#"{"error":"bare"}"#), "bare");
        assert_eq!(parse_upstream_error_message(r#"{"msg":"short"}"#), "short");
        assert_eq!(parse_upstream_error_message(""), "");
    }

    #[test]
    fn truncate_echoed_is_utf8_safe() {
        let long = "你".repeat(300);
        let out = truncate_echoed(&long);
        assert!(out.ends_with("..."));
        assert!(out.len() <= MAX_ECHOED_ERROR_TEXT + 3);
        // Cutting must not split a multi-byte char.
        assert!(std::str::from_utf8(out.as_bytes()).is_ok());
    }

    #[test]
    fn reasoning_effort_adaptation() {
        let body = br#"{"model":"m","reasoning_effort":"high"}"#;
        let message = "unsupported value: high. supported values: low, medium";
        let adapted = adapt_unsupported_reasoning_effort(body, message).expect("should adapt");
        let value: Value = serde_json::from_slice(&adapted).unwrap();
        assert_eq!(value["reasoning_effort"], "medium");

        // Nothing to change when the requested effort is supported.
        let ok = adapt_unsupported_reasoning_effort(
            br#"{"reasoning_effort":"low"}"#,
            "supported values: low, medium",
        );
        assert!(ok.is_none());

        // No supported list -> no rewrite.
        assert!(adapt_unsupported_reasoning_effort(body, "some other error").is_none());
    }

    #[test]
    fn stream_prefix_detection() {
        let error_prefix = "data: {\"error\":{\"message\":\"quota\"}}\n\n";
        assert_eq!(
            upstream_stream_error(error_prefix),
            Some(("quota".to_string(), 502))
        );
        assert!(!upstream_stream_has_output(error_prefix));

        let output_prefix = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n";
        assert!(upstream_stream_has_output(output_prefix));
        assert!(upstream_stream_error(output_prefix).is_none());

        let done_only = "data: [DONE]\n\n";
        assert!(!upstream_stream_has_output(done_only));
        assert!(upstream_stream_error(done_only).is_none());
    }

    #[test]
    fn auto_model_choice_prefers_operational_primary() {
        let channel = Channel {
            primary_model: "gpt-4o".to_string(),
            extra_models: vec!["gpt-4o-mini".to_string()],
            last_status: STATUS_OPERATIONAL.to_string(),
            ..Default::default()
        };
        assert_eq!(best_auto_model(Some(&channel)), "gpt-4o");

        // A healthy extra model beats an errored primary.
        let channel = Channel {
            primary_model: "gpt-4o".to_string(),
            extra_models: vec!["gpt-4o-mini".to_string()],
            last_status: STATUS_ERROR.to_string(),
            model_health: vec![crate::store::ModelHealth {
                model: "gpt-4o-mini".to_string(),
                status: STATUS_OPERATIONAL.to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(best_auto_model(Some(&channel)), "gpt-4o-mini");
    }
}
