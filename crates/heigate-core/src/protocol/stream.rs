//! OpenAI chat chunks -> Anthropic streaming events. Ported from
//! `internal/protocol/stream.go`.
//!
//! Text and reasoning stream immediately. Interleaved tool arguments are
//! assembled by upstream index and emitted as sequential blocks at completion.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::anthropic::{ContentBlock, STOP_END_TURN, STOP_TOOL_USE};
use super::convert::stop_reason_from_finish;
use super::openai::{chat_text, ChatChunk, ToolCall};

/// One Anthropic streaming event. The `type` key is always present.
pub type Event = Value;

pub(crate) const MAX_SSE_LINE_BYTES: usize = 1 << 20;
pub(crate) const MAX_SSE_EVENT_BYTES: usize = 1 << 20;
const MAX_PENDING_TOOL_BYTES: usize = 16 << 20;

/// Bounded SSE event decoder. Data fields in a dispatched event are joined by
/// LF, as required by the SSE format; line and event limits prevent a peer
/// from growing either buffer without bound.
#[derive(Debug, Default)]
pub(crate) struct SseEventDecoder {
    line: Vec<u8>,
    data: Vec<u8>,
    has_data: bool,
    skip_lf: bool,
    pub exceeded: bool,
}

impl SseEventDecoder {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        let mut events = Vec::new();
        if self.exceeded {
            return events;
        }
        for &byte in bytes {
            if self.skip_lf {
                self.skip_lf = false;
                if byte == b'\n' {
                    continue;
                }
            }
            match byte {
                b'\r' => {
                    self.process_line(&mut events);
                    self.skip_lf = true;
                }
                b'\n' => self.process_line(&mut events),
                _ => {
                    if self.line.len() >= MAX_SSE_LINE_BYTES {
                        self.exceeded = true;
                        self.line.clear();
                        self.data.clear();
                        self.has_data = false;
                        return events;
                    }
                    self.line.push(byte);
                }
            }
            if self.exceeded {
                break;
            }
        }
        events
    }

    /// Completes a final data event at EOF. Callers must still require their
    /// protocol's authoritative completion marker before reporting success.
    pub fn finish(&mut self) -> Vec<Vec<u8>> {
        let mut events = Vec::new();
        if self.exceeded {
            return events;
        }
        if !self.line.is_empty() {
            self.process_line(&mut events);
        }
        if self.has_data {
            events.push(std::mem::take(&mut self.data));
            self.has_data = false;
        }
        events
    }

    pub fn pending_line(&self) -> &[u8] {
        &self.line
    }

    fn process_line(&mut self, events: &mut Vec<Vec<u8>>) {
        if self.line.is_empty() {
            if self.has_data {
                events.push(std::mem::take(&mut self.data));
                self.has_data = false;
            }
            return;
        }
        if let Some(mut value) = self.line.strip_prefix(b"data:").map(<[u8]>::to_vec) {
            // Many OpenAI-compatible servers omit the blank line between
            // ordinary JSON data frames. If the previous accumulated fields
            // already form a complete JSON value, dispatch it before starting
            // the next data field; incomplete JSON fragments remain joined.
            if self.has_data
                && (self.data == b"[DONE]"
                    || serde_json::from_slice::<serde_json::Value>(&self.data).is_ok())
            {
                events.push(std::mem::take(&mut self.data));
                self.has_data = false;
            }
            // The SSE grammar removes at most one optional space after ':'.
            if value.first() == Some(&b' ') {
                value.remove(0);
            }
            let separator = usize::from(self.has_data);
            if separator.saturating_add(value.len())
                > MAX_SSE_EVENT_BYTES.saturating_sub(self.data.len())
            {
                self.exceeded = true;
                self.line.clear();
                self.data.clear();
                self.has_data = false;
                return;
            }
            if self.has_data {
                self.data.push(b'\n');
            }
            self.data.extend_from_slice(&value);
            self.has_data = true;
            if self.data == b"[DONE]"
                || serde_json::from_slice::<serde_json::Value>(&self.data).is_ok()
            {
                events.push(std::mem::take(&mut self.data));
                self.has_data = false;
            }
        }
        self.line.clear();
    }
}

/// Returns the end offset of the next SSE line, including its LF, CR, or
/// CRLF delimiter. `None` means the remaining bytes do not end a line yet.
pub(crate) fn next_sse_line_end(bytes: &[u8]) -> Option<usize> {
    for (index, byte) in bytes.iter().enumerate() {
        match byte {
            b'\n' => return Some(index + 1),
            b'\r' => {
                return Some(if bytes.get(index + 1) == Some(&b'\n') {
                    index + 2
                } else {
                    index + 1
                });
            }
            _ => {}
        }
    }
    None
}

/// Returns the event's `type` field.
pub fn event_type(event: &Event) -> &str {
    event.get("type").and_then(Value::as_str).unwrap_or("")
}

/// Renders an event as a server-sent-event frame:
///
/// ```text
/// event: <type>
/// data: <json>
///
/// ```
pub fn encode_sse(event: &Event) -> Result<String, String> {
    let kind = event_type(event);
    if kind.is_empty() {
        return Err("event has no type".to_string());
    }
    let data = serde_json::to_string(event).map_err(|err| err.to_string())?;
    Ok(format!("event: {kind}\ndata: {data}\n\n"))
}

/// Converts a sequence of OpenAI chat chunks into Anthropic streaming events.
/// Create one per response, call [`StreamState::feed`] for every chunk, then
/// [`StreamState::finish`] once the upstream stream ends.
pub struct StreamState {
    model: String,
    message_id: String,
    started: bool,
    finished: bool,
    next_index: i64,
    open_index: i64,
    open_kind: String,
    pending_tools: BTreeMap<i32, ToolCall>,
    pending_tool_bytes: usize,
    buffer_error: bool,
    input_tokens: i64,
    output_tokens: i64,
    saw_usage: bool,
    stop_reason: String,
    saw_tool_calls: bool,
}

impl StreamState {
    pub fn new(model: &str) -> Self {
        StreamState {
            model: model.to_string(),
            message_id: String::new(),
            started: false,
            finished: false,
            next_index: 0,
            open_index: -1,
            open_kind: String::new(),
            pending_tools: BTreeMap::new(),
            pending_tool_bytes: 0,
            buffer_error: false,
            input_tokens: 0,
            output_tokens: 0,
            saw_usage: false,
            stop_reason: String::new(),
            saw_tool_calls: false,
        }
    }

    /// Consumes one chunk and returns the events it produced (possibly none).
    pub fn feed(&mut self, chunk: Option<&ChatChunk>) -> Vec<Event> {
        let Some(chunk) = chunk else {
            return Vec::new();
        };
        if self.finished || self.buffer_error {
            return Vec::new();
        }

        let mut events: Vec<Event> = Vec::new();
        if let Some(usage) = &chunk.usage {
            self.saw_usage = true;
            self.input_tokens = usage.prompt_tokens;
            self.output_tokens = usage.completion_tokens;
        }
        if !self.started {
            if self.message_id.is_empty() {
                self.message_id = message_id(&chunk.id);
            }
            events.push(self.start_event());
        }

        for choice in &chunk.choices {
            if choice.index != 0 {
                continue; // only the first candidate is streamed to the client
            }
            if !choice.delta.reasoning_content.is_empty() {
                events.extend(self.ensure_block("thinking", None));
                events.push(json!({
                    "type": "content_block_delta",
                    "index": self.open_index,
                    "delta": {"type": "thinking_delta", "thinking": choice.delta.reasoning_content},
                }));
                let text = choice.delta.reasoning_content.clone();
                self.count_tokens(&text);
            }
            let text = chat_text(choice.delta.content.as_ref());
            if !text.is_empty() {
                events.extend(self.ensure_block("text", None));
                events.push(json!({
                    "type": "content_block_delta",
                    "index": self.open_index,
                    "delta": {"type": "text_delta", "text": text},
                }));
                self.count_tokens(&text);
            }
            for (position, call) in choice.delta.tool_calls.iter().enumerate() {
                self.saw_tool_calls = true;
                let idx = call.index.unwrap_or(position as i32);
                let added = call
                    .id
                    .len()
                    .saturating_add(call.function.name.len())
                    .saturating_add(call.function.arguments.len());
                if added > MAX_PENDING_TOOL_BYTES.saturating_sub(self.pending_tool_bytes)
                    || (self.pending_tools.len() >= 1024 && !self.pending_tools.contains_key(&idx))
                {
                    self.buffer_error = true;
                    self.pending_tools.clear();
                    return Vec::new();
                }
                self.pending_tool_bytes += added;
                // OpenAI interleaves parallel tool arguments; Anthropic clients
                // expect each block to finish before the next one starts.
                let pending = self.pending_tools.entry(idx).or_default();
                if !call.id.is_empty() {
                    pending.id.clone_from(&call.id);
                }
                pending.function.name.push_str(&call.function.name);
                pending
                    .function
                    .arguments
                    .push_str(&call.function.arguments);
                self.count_tokens(&call.function.arguments);
            }
            if let Some(finish) = &choice.finish_reason {
                if !finish.is_empty() {
                    self.stop_reason = stop_reason_from_finish(finish, self.saw_tool_calls);
                }
            }
        }

        events
    }

    /// Closes any open block and emits `message_delta` + `message_stop`. Safe
    /// to call even if no chunk was ever fed.
    pub fn finish(&mut self) -> Vec<Event> {
        if self.finished || self.buffer_error {
            return Vec::new();
        }
        self.finished = true;

        let mut events: Vec<Event> = Vec::new();
        if !self.started {
            if self.message_id.is_empty() {
                self.message_id = message_id("");
            }
            events.push(self.start_event());
        }
        for (_, call) in std::mem::take(&mut self.pending_tools) {
            events.extend(self.ensure_block("tool_use", Some(&call)));
            // Keep synthesized SSE lines bounded even when arguments arrived in many deltas.
            let mut arguments = call.function.arguments.as_str();
            while !arguments.is_empty() {
                let mut end = arguments.len().min(32 << 10);
                while !arguments.is_char_boundary(end) {
                    end -= 1;
                }
                events.push(json!({
                    "type": "content_block_delta",
                    "index": self.open_index,
                    "delta": {"type": "input_json_delta", "partial_json": &arguments[..end]},
                }));
                arguments = &arguments[end..];
            }
        }
        events.extend(self.close_block());

        let mut stop = self.stop_reason.clone();
        if stop.is_empty() {
            stop = if self.saw_tool_calls {
                STOP_TOOL_USE.to_string()
            } else {
                STOP_END_TURN.to_string()
            };
        }
        let mut usage = json!({"output_tokens": self.output_tokens});
        // Chat upstreams report input usage at the end of the stream.
        if self.saw_usage {
            usage["input_tokens"] = json!(self.input_tokens);
        }
        events.push(json!({
            "type": "message_delta",
            "delta": {"stop_reason": stop, "stop_sequence": null},
            "usage": usage,
        }));
        events.push(json!({"type": "message_stop"}));
        events
    }

    /// Returns `(input, output, exact)`; `exact` is true when the upstream
    /// reported usage itself rather than HeiGate estimating it.
    pub fn usage(&self) -> (i64, i64, bool) {
        (self.input_tokens, self.output_tokens, self.saw_usage)
    }

    pub(crate) fn buffer_limit_exceeded(&self) -> bool {
        self.buffer_error
    }

    fn start_event(&mut self) -> Event {
        self.started = true;
        json!({
            "type": "message_start",
            "message": {
                "id": self.message_id,
                "type": "message",
                "role": "assistant",
                "model": self.model,
                "content": [],
                "stop_reason": null,
                "stop_sequence": null,
                "usage": {"input_tokens": self.input_tokens, "output_tokens": 0},
            }
        })
    }

    fn ensure_block(&mut self, kind: &str, call: Option<&ToolCall>) -> Vec<Event> {
        if self.open_index >= 0 && self.open_kind == kind && kind != "tool_use" {
            return Vec::new();
        }

        let mut events = self.close_block();
        let index = self.next_index;
        self.next_index += 1;
        self.open_index = index;
        self.open_kind = kind.to_string();

        let block = match kind {
            "text" => json!({"type": "text", "text": ""}),
            "thinking" => json!({"type": "thinking", "thinking": ""}),
            "tool_use" => {
                let (id, name) = match call {
                    Some(call) => (call.id.clone(), call.function.name.clone()),
                    None => (String::new(), String::new()),
                };
                json!({
                    "type": "tool_use",
                    "id": tool_use_id(&id, index as usize),
                    "name": name,
                    "input": {},
                })
            }
            _ => json!({"type": kind}),
        };
        events.push(json!({
            "type": "content_block_start",
            "index": index,
            "content_block": block,
        }));
        events
    }

    fn close_block(&mut self) -> Vec<Event> {
        if self.open_index < 0 {
            return Vec::new();
        }
        let event = json!({"type": "content_block_stop", "index": self.open_index});
        self.open_index = -1;
        self.open_kind = String::new();
        vec![event]
    }

    /// Keeps a rough running output estimate until the upstream reports exact
    /// usage.
    fn count_tokens(&mut self, text: &str) {
        if self.saw_usage {
            return;
        }
        self.output_tokens += estimate_tokens(text);
    }
}

/// A cheap heuristic used when a provider omits usage: CJK characters count as
/// one token each, other text as roughly four characters per token.
pub fn estimate_tokens(text: &str) -> i64 {
    if text.is_empty() {
        return 0;
    }
    let mut wide = 0i64;
    let mut ascii = 0i64;
    for ch in text.chars() {
        if ch as u32 > 0x7F {
            wide += 1;
        } else {
            ascii += 1;
        }
    }
    let n = wide + (ascii + 3) / 4;
    n.max(1)
}

/// Extracts the JSON payload from a `data: ...` line. `done` is true for the
/// `[DONE]` sentinel; `ok` is false for non-data lines.
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

fn message_id(id: &str) -> String {
    let id = id.trim();
    if id.is_empty() {
        return format!("msg_{}", random_id(12));
    }
    if id.starts_with("msg_") {
        return id.to_string();
    }
    format!("msg_{id}")
}

fn tool_use_id(id: &str, index: usize) -> String {
    let id = id.trim();
    if !id.is_empty() {
        return id.to_string();
    }
    format!("toolu_{}_{index}", random_id(8))
}

/// Hex-encodes `n` random bytes; falls back to zeros on RNG failure.
pub fn random_id(n: usize) -> String {
    let mut buf = vec![0u8; n];
    if getrandom::fill(&mut buf).is_err() {
        return "0".repeat(n * 2);
    }
    let mut out = String::with_capacity(n * 2);
    for byte in buf {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Re-exported so callers can build an empty text block without importing
/// `anthropic` directly.
pub fn empty_text_block() -> ContentBlock {
    ContentBlock {
        r#type: "text".to_string(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    //! Ports the streaming cases from `internal/protocol/protocol_test.go`.

    use super::*;
    use crate::protocol::openai::{ChatMessage, ChunkChoice, ToolCall};

    fn chunk(delta: ChatMessage, finish: Option<&str>) -> ChatChunk {
        ChatChunk {
            id: "chatcmpl-1".to_string(),
            choices: vec![ChunkChoice {
                index: 0,
                delta,
                finish_reason: finish.map(str::to_string),
            }],
            ..Default::default()
        }
    }

    #[test]
    fn estimate_tokens_counts_cjk_as_one() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1); // (4+3)/4 = 1
        assert_eq!(estimate_tokens("abcdefgh"), 2);
        assert_eq!(estimate_tokens("你好世界"), 4); // four wide chars
        assert_eq!(estimate_tokens("a"), 1); // minimum one for non-empty
    }

    #[test]
    fn tool_argument_and_tool_count_buffers_are_bounded() {
        for many_calls in [false, true] {
            let mut state = StreamState::new("model");
            for index in 0..=1024 {
                let tool = ToolCall {
                    index: Some(if many_calls { index } else { 0 }),
                    function: super::super::openai::FunctionCall {
                        arguments: if many_calls {
                            "{}".into()
                        } else {
                            "x".repeat(256 << 10)
                        },
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let chunk = ChatChunk {
                    choices: vec![super::super::openai::ChunkChoice {
                        delta: super::super::openai::ChatMessage {
                            tool_calls: vec![tool],
                            ..Default::default()
                        },
                        ..Default::default()
                    }],
                    ..Default::default()
                };
                state.feed(Some(&chunk));
                if state.buffer_limit_exceeded() {
                    break;
                }
            }
            assert!(state.buffer_limit_exceeded());
            assert!(state.pending_tools.is_empty());
            assert!(state.finish().is_empty());
        }
    }

    #[test]
    fn large_tool_arguments_are_split_into_bounded_utf8_safe_events() {
        let arguments = "\"你好\\\n".repeat(200_000);
        let mut state = StreamState::new("model");
        state.feed(Some(&chunk(
            ChatMessage {
                tool_calls: vec![ToolCall {
                    id: "call-1".into(),
                    function: super::super::openai::FunctionCall {
                        name: "tool".into(),
                        arguments: arguments.clone(),
                    },
                    ..Default::default()
                }],
                ..Default::default()
            },
            Some("tool_calls"),
        )));
        let events = state.finish();
        let mut received = String::new();
        for event in &events {
            assert!(encode_sse(event).unwrap().len() < MAX_SSE_LINE_BYTES);
            if let Some(part) = event["delta"]["partial_json"].as_str() {
                received.push_str(part);
            }
        }
        assert_eq!(received, arguments);
    }

    #[test]
    fn parse_sse_line_handles_data_and_done() {
        assert_eq!(
            parse_sse_line("data: {\"a\":1}"),
            ("{\"a\":1}".to_string(), false, true)
        );
        assert_eq!(parse_sse_line("data: [DONE]"), (String::new(), true, true));
        assert_eq!(parse_sse_line("event: ping"), (String::new(), false, false));
        assert_eq!(parse_sse_line("data:"), (String::new(), false, false));
    }

    #[test]
    fn stream_switches_blocks_between_kinds() {
        let mut state = StreamState::new("claude");
        let mut types = Vec::new();

        let first = state.feed(Some(&chunk(
            ChatMessage {
                role: "assistant".to_string(),
                reasoning_content: "hmm".to_string(),
                ..Default::default()
            },
            None,
        )));
        for event in &first {
            types.push(event_type(event).to_string());
        }

        let second = state.feed(Some(&chunk(
            ChatMessage {
                role: "assistant".to_string(),
                content: Some(json!("answer")),
                ..Default::default()
            },
            Some("stop"),
        )));
        for event in &second {
            types.push(event_type(event).to_string());
        }

        let third = state.finish();
        for event in &third {
            types.push(event_type(event).to_string());
        }

        // message_start -> thinking block open + delta -> close + text block open + delta
        // -> close -> message_delta -> message_stop
        assert_eq!(
            types,
            vec![
                "message_start",
                "content_block_start",
                "content_block_delta",
                "content_block_stop",
                "content_block_start",
                "content_block_delta",
                "content_block_stop",
                "message_delta",
                "message_stop",
            ]
        );

        let (_, output, exact) = state.usage();
        assert!(!exact);
        assert!(output > 0);
    }

    #[test]
    fn upstream_usage_overrides_the_estimate() {
        let mut state = StreamState::new("claude");
        state.feed(Some(&chunk(
            ChatMessage {
                role: "assistant".to_string(),
                content: Some(json!("hello there")),
                ..Default::default()
            },
            None,
        )));
        let mut with_usage = chunk(
            ChatMessage {
                role: "assistant".to_string(),
                ..Default::default()
            },
            Some("stop"),
        );
        with_usage.usage = Some(crate::protocol::ChatUsage {
            prompt_tokens: 11,
            completion_tokens: 22,
            total_tokens: 33,
        });
        state.feed(Some(&with_usage));
        let (input, output, exact) = state.usage();
        assert!(exact);
        assert_eq!(input, 11);
        assert_eq!(output, 22);
        let events = state.finish();
        let usage = &events
            .iter()
            .find(|event| event_type(event) == "message_delta")
            .unwrap()["usage"];
        assert_eq!(usage["input_tokens"], 11);
        assert_eq!(usage["output_tokens"], 22);
    }

    #[test]
    fn first_chunk_usage_is_available_in_message_start() {
        let mut state = StreamState::new("claude");
        let first = ChatChunk {
            usage: Some(crate::protocol::ChatUsage {
                prompt_tokens: 100,
                completion_tokens: 20,
                total_tokens: 120,
            }),
            ..Default::default()
        };
        let events = state.feed(Some(&first));
        assert_eq!(events[0]["message"]["usage"]["input_tokens"], 100);
    }

    #[test]
    fn tool_calls_open_blocks_keyed_by_upstream_index() {
        let mut state = StreamState::new("claude");
        let mut events = state.feed(Some(&chunk(
            ChatMessage {
                role: "assistant".to_string(),
                tool_calls: vec![ToolCall {
                    index: Some(0),
                    id: "call_1".to_string(),
                    r#type: "function".to_string(),
                    function: crate::protocol::FunctionCall {
                        name: "lookup".to_string(),
                        arguments: "{\"q\":".to_string(),
                    },
                }],
                ..Default::default()
            },
            None,
        )));
        events.extend(state.finish());
        let kinds: Vec<String> = events
            .iter()
            .take(3)
            .map(|e| event_type(e).to_string())
            .collect();
        assert_eq!(
            kinds,
            vec![
                "message_start",
                "content_block_start",
                "content_block_delta"
            ]
        );
        assert_eq!(events[1]["content_block"]["type"], "tool_use");
        assert_eq!(events[1]["content_block"]["name"], "lookup");
    }

    #[test]
    fn encode_sse_requires_a_type() {
        assert!(encode_sse(&json!({"type": "message_stop"})).is_ok());
        assert!(encode_sse(&json!({"foo": 1})).is_err());
        let frame = encode_sse(&json!({"type": "message_stop"})).unwrap();
        assert!(frame.starts_with("event: message_stop\ndata: "));
        assert!(frame.ends_with("\n\n"));
    }
}
