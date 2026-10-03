//! Anthropic Messages API wire types. Ported from `internal/protocol/anthropic.go`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::is_false;

/// The body of `POST /v1/messages`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
pub struct MessagesRequest {
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub messages: Vec<Message>,
    /// A string or an array of content blocks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop_sequences: Vec<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub stream: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<Value>,
}

/// One conversation turn. `content` is either a plain string or an array of
/// content blocks.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
pub struct Message {
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub content: Option<Value>,
}

/// A union of every block type we understand. Unused fields are omitted when
/// serialised.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
pub struct ContentBlock {
    #[serde(rename = "type", default)]
    pub r#type: String,

    // text / thinking
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thinking: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub signature: String,

    // image
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ImageSource>,

    // tool_use
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,

    // tool_result
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tool_use_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Value>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_error: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<Value>,
}

/// Either inline base64 data or a URL.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
pub struct ImageSource {
    #[serde(rename = "type", default)]
    pub r#type: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub media_type: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub data: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
}

/// A client-defined function tool. Server-side tools carry a `type` and no
/// `input_schema` and are ignored during translation.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
pub struct Tool {
    #[serde(rename = "type", default, skip_serializing_if = "String::is_empty")]
    pub r#type: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
}

/// Controls how the model may use tools.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
pub struct ToolChoice {
    #[serde(rename = "type", default)]
    pub r#type: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub disable_parallel_tool_use: bool,
}

/// The optional end-user identifier.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
pub struct Metadata {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub user_id: String,
}

/// The body returned by `POST /v1/messages`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MessagesResponse {
    pub id: String,
    #[serde(rename = "type")]
    pub r#type: String,
    pub role: String,
    pub model: String,
    pub content: Vec<ContentBlock>,
    pub stop_reason: String,
    pub stop_sequence: Option<String>,
    pub usage: MessagesUsage,
}

/// The Anthropic token accounting block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MessagesUsage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_input_tokens: i64,
    pub cache_read_input_tokens: i64,
}

pub const STOP_END_TURN: &str = "end_turn";
pub const STOP_MAX_TOKENS: &str = "max_tokens";
pub const STOP_STOP_SEQUENCE: &str = "stop_sequence";
pub const STOP_TOOL_USE: &str = "tool_use";

/// Decodes a message-content (or `tool_result` content) value into blocks. A
/// plain string becomes a single text block.
pub fn blocks(raw: Option<&Value>) -> Vec<ContentBlock> {
    match raw {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::String(text)) => {
            if text.is_empty() {
                Vec::new()
            } else {
                vec![ContentBlock {
                    r#type: "text".to_string(),
                    text: text.clone(),
                    ..Default::default()
                }]
            }
        }
        Some(Value::Array(_)) => serde_json::from_value(raw.cloned().unwrap()).unwrap_or_default(),
        Some(_) => Vec::new(),
    }
}

/// Flattens the system prompt into a single string.
pub fn system_text(raw: Option<&Value>) -> String {
    let mut parts: Vec<String> = Vec::new();
    for block in blocks(raw) {
        if block.r#type == "text" && !block.text.is_empty() {
            parts.push(block.text);
        }
    }
    parts.join("\n\n")
}
