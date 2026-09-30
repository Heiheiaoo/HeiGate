//! Translates between the Anthropic Messages API and the OpenAI Chat
//! Completions API so a client speaking one dialect can be served by an
//! upstream speaking the other. Ported from the Go `internal/protocol`.
//!
//! Only the subset needed by a coding-assistant gateway is modelled: text,
//! images, tool definitions, tool calls, tool results, thinking/reasoning text,
//! stop reasons and usage.

pub mod anthropic;
pub mod convert;
pub mod openai;
pub mod stream;

pub use anthropic::{
    blocks, system_text, ContentBlock, ImageSource, Message, MessagesRequest, MessagesResponse,
    MessagesUsage, Metadata, Tool, ToolChoice, STOP_END_TURN, STOP_MAX_TOKENS, STOP_STOP_SEQUENCE,
    STOP_TOOL_USE,
};
pub use convert::{chat_to_messages, messages_to_chat, ConvertError};
pub use openai::{
    chat_text, ChatChoice, ChatChunk, ChatContentPart, ChatMessage, ChatRequest, ChatResponse,
    ChatTool, ChatUsage, ChunkChoice, FunctionCall, FunctionSpec, ImageURL, StreamOptions, ToolCall,
    FINISH_CONTENT_FILTER, FINISH_LENGTH, FINISH_STOP, FINISH_TOOL_CALLS,
};
pub use stream::{encode_sse, estimate_tokens, parse_sse_line, Event, StreamState};

use serde_json::Value;

/// `serde` helper: omit a boolean field when it is false, matching Go's
/// `omitempty`.
pub(crate) fn is_false(value: &bool) -> bool {
    !*value
}

/// Trims a raw JSON value the way Go's `trimRaw` trims a `json.RawMessage`.
pub(crate) fn trim_raw(raw: Option<&Value>) -> Option<&Value> {
    match raw {
        None | Some(Value::Null) => None,
        Some(value) => Some(value),
    }
}
