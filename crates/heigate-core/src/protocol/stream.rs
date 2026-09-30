//! OpenAI chat chunks -> Anthropic streaming events. Ported from
//! `internal/protocol/stream.go`.
//!
//! A new content block is opened whenever the kind changes or a tool call with
//! a new upstream index appears.

use std::collections::HashMap;

use serde_json::{json, Value};

use super::anthropic::{ContentBlock, STOP_END_TURN, STOP_TOOL_USE};
use super::convert::stop_reason_from_finish;
use super::openai::{chat_text, ChatChunk, ToolCall};

/// One Anthropic streaming event. The `type` key is always present.
pub type Event = Value;

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
    tool_blocks: HashMap<i32, i64>,
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
            tool_blocks: HashMap::new(),
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
        if self.finished {
            return Vec::new();
        }

        let mut events: Vec<Event> = Vec::new();
        if !self.started {
            if self.message_id.is_empty() {
                self.message_id = message_id(&chunk.id);
            }
            events.push(self.start_event());
        }

        if let Some(usage) = &chunk.usage {
            self.saw_usage = true;
            if usage.prompt_tokens > 0 {
                self.input_tokens = usage.prompt_tokens;
            }
            if usage.completion_tokens > 0 {
                self.output_tokens = usage.completion_tokens;
            }
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
                if !self.tool_blocks.contains_key(&idx) {
                    events.extend(self.ensure_block("tool_use", Some(call)));
                    self.tool_blocks.insert(idx, self.open_index);
                }
                if !call.function.arguments.is_empty() {
                    let block_index = self.tool_blocks[&idx];
                    events.push(json!({
                        "type": "content_block_delta",
                        "index": block_index,
                        "delta": {"type": "input_json_delta", "partial_json": call.function.arguments},
                    }));
                    let args = call.function.arguments.clone();
                    self.count_tokens(&args);
                }
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
        if self.finished {
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
        events.extend(self.close_block());

        let mut stop = self.stop_reason.clone();
        if stop.is_empty() {
            stop = if self.saw_tool_calls {
                STOP_TOOL_USE.to_string()
            } else {
                STOP_END_TURN.to_string()
            };
        }
        events.push(json!({
            "type": "message_delta",
            "delta": {"stop_reason": stop, "stop_sequence": null},
            "usage": {"output_tokens": self.output_tokens},
        }));
        events.push(json!({"type": "message_stop"}));
        events
    }

    /// Returns `(input, output, exact)`; `exact` is true when the upstream
    /// reported usage itself rather than HeiGate estimating it.
    pub fn usage(&self) -> (i64, i64, bool) {
        (self.input_tokens, self.output_tokens, self.saw_usage)
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
    }

    #[test]
    fn tool_calls_open_blocks_keyed_by_upstream_index() {
        let mut state = StreamState::new("claude");
        let events = state.feed(Some(&chunk(
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
        let kinds: Vec<String> = events.iter().map(|e| event_type(e).to_string()).collect();
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
