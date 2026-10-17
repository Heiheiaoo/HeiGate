//! OpenAI Chat Completions API wire types. Ported from
//! `internal/protocol/openai.go`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{is_false, trim_raw};

/// The body of `POST /v1/chat/completions`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
pub struct ChatRequest {
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop: Vec<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub stream: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<StreamOptions>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ChatTool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub user: String,
}

/// Asks the upstream to append a usage chunk to the stream.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
pub struct StreamOptions {
    pub include_usage: bool,
}

/// One message in the OpenAI format. `content` is a string or an array of
/// content parts; it is omitted entirely for tool-call-only assistant turns.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
pub struct ChatMessage {
    #[serde(default)]
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Value>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tool_call_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reasoning_content: String,
}

/// A multimodal content element.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
pub struct ChatContentPart {
    #[serde(rename = "type")]
    pub r#type: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_url: Option<ImageURL>,
}

/// Wraps an http(s) or `data:` URL.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
pub struct ImageURL {
    pub url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

/// Declares a function the model may call.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
pub struct ChatTool {
    #[serde(rename = "type")]
    pub r#type: String,
    pub function: FunctionSpec,
}

/// The JSON-schema description of a function tool.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
pub struct FunctionSpec {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

/// A function invocation requested by the assistant. `index` is only present in
/// streaming deltas.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
pub struct ToolCall {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<i32>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    #[serde(rename = "type", default, skip_serializing_if = "String::is_empty")]
    pub r#type: String,
    #[serde(default)]
    pub function: FunctionCall,
}

/// Names the function and carries its JSON-encoded arguments.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
pub struct FunctionCall {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub arguments: String,
}

/// A non-streaming completion.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
pub struct ChatResponse {
    #[serde(default)]
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub object: String,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub created: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(default)]
    pub choices: Vec<ChatChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<ChatUsage>,
}

/// One candidate answer.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
pub struct ChatChoice {
    #[serde(default)]
    pub index: i32,
    #[serde(default)]
    pub message: ChatMessage,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub finish_reason: String,
}

/// One streaming SSE payload.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
pub struct ChatChunk {
    #[serde(default)]
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub object: String,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub created: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(default)]
    pub choices: Vec<ChunkChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<ChatUsage>,
}

/// Carries an incremental delta.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
pub struct ChunkChoice {
    #[serde(default)]
    pub index: i32,
    #[serde(default)]
    pub delta: ChatMessage,
    /// Present (possibly null) on every chunk; only the value is meaningful.
    #[serde(default)]
    pub finish_reason: Option<String>,
}

/// The OpenAI token accounting block.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
pub struct ChatUsage {
    #[serde(default)]
    pub prompt_tokens: i64,
    #[serde(default)]
    pub completion_tokens: i64,
    #[serde(default)]
    pub total_tokens: i64,
}

pub const FINISH_STOP: &str = "stop";
pub const FINISH_LENGTH: &str = "length";
pub const FINISH_TOOL_CALLS: &str = "tool_calls";
pub const FINISH_CONTENT_FILTER: &str = "content_filter";

/// Flattens a `ChatMessage.content` value (string or parts) to text.
pub fn chat_text(raw: Option<&Value>) -> String {
    let Some(raw) = trim_raw(raw) else {
        return String::new();
    };
    if let Value::String(text) = raw {
        return text.clone();
    }
    if let Value::Array(parts) = raw {
        let parsed: Vec<ChatContentPart> =
            serde_json::from_value(Value::Array(parts.clone())).unwrap_or_default();
        let mut out = String::new();
        for part in parsed {
            if part.r#type == "text" || part.r#type.is_empty() {
                out.push_str(&part.text);
            }
        }
        return out;
    }
    String::new()
}

/// Wraps a string as a JSON value, matching Go's `rawString`.
pub fn raw_string(text: &str) -> Value {
    Value::String(text.to_string())
}

fn is_zero_i64(value: &i64) -> bool {
    *value == 0
}
