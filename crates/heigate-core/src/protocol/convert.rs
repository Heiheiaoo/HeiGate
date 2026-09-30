//! Anthropic -> OpenAI (and back) request/response conversion. Ported from
//! `internal/protocol/convert.go`.

use serde_json::{json, Value};

use super::anthropic::{blocks, system_text, ContentBlock, ImageSource, Message, MessagesRequest};
use super::openai::{
    raw_string, ChatContentPart, ChatMessage, ChatRequest, ChatResponse, ChatTool, FunctionCall,
    FunctionSpec, ImageURL, StreamOptions, ToolCall, FINISH_LENGTH, FINISH_STOP, FINISH_TOOL_CALLS,
};
use super::stream::random_id;
use super::{STOP_END_TURN, STOP_MAX_TOKENS, STOP_TOOL_USE};

/// Errors from protocol conversion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConvertError {
    /// The request carried no conversation turns.
    NoMessages,
    UnsupportedRole {
        index: usize,
        role: String,
    },
}

impl std::fmt::Display for ConvertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConvertError::NoMessages => write!(f, "messages must not be empty"),
            ConvertError::UnsupportedRole { index, role } => {
                write!(f, "messages[{index}]: unsupported role {role:?}")
            }
        }
    }
}

impl std::error::Error for ConvertError {}

/// Converts an Anthropic Messages request into an OpenAI Chat Completions
/// request. See the Go original for the full field mapping.
pub fn messages_to_chat(req: Option<&MessagesRequest>) -> Result<ChatRequest, ConvertError> {
    let req = req.ok_or(ConvertError::NoMessages)?;
    if req.messages.is_empty() {
        return Err(ConvertError::NoMessages);
    }

    let mut out = ChatRequest {
        model: req.model.clone(),
        temperature: req.temperature,
        top_p: req.top_p,
        stream: req.stream,
        ..Default::default()
    };
    if let Some(max_tokens) = req.max_tokens {
        if max_tokens > 0 {
            out.max_tokens = Some(max_tokens);
        }
    }
    if !req.stop_sequences.is_empty() {
        out.stop = req.stop_sequences.clone();
    }
    if req.stream {
        out.stream_options = Some(StreamOptions {
            include_usage: true,
        });
    }
    if let Some(metadata) = &req.metadata {
        if !metadata.user_id.is_empty() {
            out.user = metadata.user_id.clone();
        }
    }

    let system = system_text(req.system.as_ref());
    if !system.is_empty() {
        out.messages.push(ChatMessage {
            role: "system".to_string(),
            content: Some(raw_string(&system)),
            ..Default::default()
        });
    }

    for (index, message) in req.messages.iter().enumerate() {
        match message.role.as_str() {
            "user" => out.messages.extend(user_turn_to_chat(message)),
            "assistant" => out.messages.push(assistant_turn_to_chat(message)),
            other => {
                return Err(ConvertError::UnsupportedRole {
                    index,
                    role: other.to_string(),
                })
            }
        }
    }

    for tool in &req.tools {
        if tool.name.is_empty() || tool.input_schema.is_none() {
            continue; // server-side tools have no client schema
        }
        out.tools.push(ChatTool {
            r#type: "function".to_string(),
            function: FunctionSpec {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: tool.input_schema.clone(),
            },
        });
    }

    if let Some(choice) = &req.tool_choice {
        if !out.tools.is_empty() {
            out.tool_choice = Some(match choice.r#type.as_str() {
                "any" => json!("required"),
                "tool" => json!({
                    "type": "function",
                    "function": {"name": choice.name},
                }),
                "none" => json!("none"),
                _ => json!("auto"),
            });
            if choice.disable_parallel_tool_use {
                out.parallel_tool_calls = Some(false);
            }
        }
    }

    Ok(out)
}

/// Converts a user turn, emitting tool replies first because OpenAI requires
/// tool responses to immediately follow the assistant tool call.
pub fn user_turn_to_chat(message: &Message) -> Vec<ChatMessage> {
    let mut tool_replies: Vec<ChatMessage> = Vec::new();
    let mut parts: Vec<ChatContentPart> = Vec::new();

    for block in blocks(message.content.as_ref()) {
        match block.r#type.as_str() {
            "text" => {
                if !block.text.is_empty() {
                    parts.push(ChatContentPart {
                        r#type: "text".to_string(),
                        text: block.text,
                        image_url: None,
                    });
                }
            }
            "image" => {
                let url = image_to_data_url(block.source.as_ref());
                if !url.is_empty() {
                    parts.push(ChatContentPart {
                        r#type: "image_url".to_string(),
                        text: String::new(),
                        image_url: Some(ImageURL {
                            url,
                            detail: String::new(),
                        }),
                    });
                }
            }
            "tool_result" => {
                tool_replies.push(ChatMessage {
                    role: "tool".to_string(),
                    content: Some(raw_string(&tool_result_text(&block))),
                    tool_call_id: block.tool_use_id.clone(),
                    ..Default::default()
                });
            }
            _ => {}
        }
    }

    let mut out = tool_replies;
    if !parts.is_empty() {
        out.push(ChatMessage {
            role: "user".to_string(),
            content: Some(encode_parts(&parts)),
            ..Default::default()
        });
    } else if out.is_empty() {
        // An empty user turn is still a turn; keep the conversation shape valid.
        out.push(ChatMessage {
            role: "user".to_string(),
            content: Some(raw_string("")),
            ..Default::default()
        });
    }
    out
}

/// Converts an assistant turn, dropping thinking blocks (provider-specific and
/// not replayable).
pub fn assistant_turn_to_chat(message: &Message) -> ChatMessage {
    let mut out = ChatMessage {
        role: "assistant".to_string(),
        ..Default::default()
    };
    let mut text = String::new();

    for block in blocks(message.content.as_ref()) {
        match block.r#type.as_str() {
            "text" => text.push_str(&block.text),
            "tool_use" => {
                let args = block
                    .input
                    .as_ref()
                    .map(|value| value.to_string())
                    .unwrap_or_default();
                let args = args.trim();
                let args = if args.is_empty() { "{}" } else { args };
                out.tool_calls.push(ToolCall {
                    index: None,
                    id: block.id.clone(),
                    r#type: "function".to_string(),
                    function: FunctionCall {
                        name: block.name.clone(),
                        arguments: args.to_string(),
                    },
                });
            }
            _ => {}
        }
    }

    if !text.is_empty() || out.tool_calls.is_empty() {
        out.content = Some(raw_string(&text));
    }
    out
}

/// Collapses a single text part into a plain string, which every
/// OpenAI-compatible provider accepts, and keeps the array form otherwise.
fn encode_parts(parts: &[ChatContentPart]) -> Value {
    if parts.len() == 1 && parts[0].r#type == "text" {
        return raw_string(&parts[0].text);
    }
    serde_json::to_value(parts).unwrap_or_else(|_| Value::Array(vec![]))
}

fn image_to_data_url(source: Option<&ImageSource>) -> String {
    let Some(source) = source else {
        return String::new();
    };
    match source.r#type.as_str() {
        "url" => source.url.clone(),
        "base64" => {
            if source.data.is_empty() {
                return String::new();
            }
            let media_type = if source.media_type.is_empty() {
                "image/png"
            } else {
                &source.media_type
            };
            format!("data:{media_type};base64,{}", source.data)
        }
        _ => String::new(),
    }
}

/// Flattens a `tool_result` payload to text. Nested images are summarised
/// because the OpenAI `tool` role only accepts text.
fn tool_result_text(block: &ContentBlock) -> String {
    let Some(raw) = super::trim_raw(block.content.as_ref()) else {
        return String::new();
    };
    if let Value::String(text) = raw {
        return text.clone();
    }

    let mut out = String::new();
    for inner in blocks(Some(raw)) {
        match inner.r#type.as_str() {
            "text" => out.push_str(&inner.text),
            "image" => out.push_str("[image omitted]"),
            _ => {}
        }
    }
    if out.is_empty() {
        return raw.to_string();
    }
    out
}

/// Converts a non-streaming Chat Completions response into an Anthropic
/// Messages response. `model` is echoed back verbatim.
pub fn chat_to_messages(resp: &ChatResponse, model: &str) -> super::MessagesResponse {
    let mut out = super::MessagesResponse {
        id: message_id(&resp.id),
        r#type: "message".to_string(),
        role: "assistant".to_string(),
        model: model.to_string(),
        stop_reason: STOP_END_TURN.to_string(),
        stop_sequence: None,
        content: Vec::new(),
        usage: Default::default(),
    };

    if let Some(usage) = &resp.usage {
        out.usage = super::MessagesUsage {
            input_tokens: usage.prompt_tokens,
            output_tokens: usage.completion_tokens,
            ..Default::default()
        };
    }

    let Some(choice) = resp.choices.first() else {
        out.content.push(ContentBlock {
            r#type: "text".to_string(),
            ..Default::default()
        });
        return out;
    };

    let reasoning = choice.message.reasoning_content.trim();
    if !reasoning.is_empty() {
        out.content.push(ContentBlock {
            r#type: "thinking".to_string(),
            thinking: reasoning.to_string(),
            ..Default::default()
        });
    }
    let text = super::chat_text(choice.message.content.as_ref());
    if !text.is_empty() {
        out.content.push(ContentBlock {
            r#type: "text".to_string(),
            text,
            ..Default::default()
        });
    }
    for (index, call) in choice.message.tool_calls.iter().enumerate() {
        out.content.push(ContentBlock {
            r#type: "tool_use".to_string(),
            id: tool_use_id(&call.id, index),
            name: call.function.name.clone(),
            input: Some(arguments_to_input(&call.function.arguments)),
            ..Default::default()
        });
    }
    if out.content.is_empty() {
        out.content.push(ContentBlock {
            r#type: "text".to_string(),
            ..Default::default()
        });
    }
    out.stop_reason =
        stop_reason_from_finish(&choice.finish_reason, !choice.message.tool_calls.is_empty());
    out
}

/// Maps an OpenAI finish reason to an Anthropic stop reason.
pub fn stop_reason_from_finish(finish: &str, has_tool_calls: bool) -> String {
    match finish {
        FINISH_TOOL_CALLS => STOP_TOOL_USE.to_string(),
        FINISH_LENGTH => STOP_MAX_TOKENS.to_string(),
        FINISH_STOP | "" => {
            if has_tool_calls {
                STOP_TOOL_USE.to_string()
            } else {
                STOP_END_TURN.to_string()
            }
        }
        _ => {
            if has_tool_calls {
                STOP_TOOL_USE.to_string()
            } else {
                STOP_END_TURN.to_string()
            }
        }
    }
}

/// Parses the JSON arguments string; malformed or empty arguments become an
/// empty object so the client always receives valid JSON.
pub fn arguments_to_input(args: &str) -> Value {
    let args = args.trim();
    if args.is_empty() {
        return json!({});
    }
    match serde_json::from_str::<Value>(args) {
        Ok(value @ Value::Object(_)) => value,
        _ => json!({}),
    }
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

#[cfg(test)]
mod tests {
    //! Ports the conversion cases from `internal/protocol/protocol_test.go`.

    use super::*;
    use crate::protocol::anthropic::MessagesUsage;

    fn user_message(content: Value) -> Message {
        Message {
            role: "user".to_string(),
            content: Some(content),
        }
    }

    #[test]
    fn rejects_empty_conversation() {
        let req = MessagesRequest {
            model: "claude".to_string(),
            messages: vec![],
            ..Default::default()
        };
        assert_eq!(messages_to_chat(Some(&req)), Err(ConvertError::NoMessages));
        assert_eq!(messages_to_chat(None), Err(ConvertError::NoMessages));
    }

    #[test]
    fn system_prompt_becomes_a_leading_message() {
        let req = MessagesRequest {
            model: "claude".to_string(),
            system: Some(json!("be brief")),
            messages: vec![user_message(json!("hi"))],
            ..Default::default()
        };
        let chat = messages_to_chat(Some(&req)).unwrap();
        assert_eq!(chat.messages[0].role, "system");
        assert_eq!(chat.messages[0].content, Some(json!("be brief")));
        assert_eq!(chat.messages[1].role, "user");
    }

    #[test]
    fn tool_results_are_emitted_before_user_content() {
        let content = json!([
            {"type": "text", "text": "here is the result"},
            {"type": "tool_result", "tool_use_id": "toolu_1", "content": "42"}
        ]);
        let req = MessagesRequest {
            model: "claude".to_string(),
            messages: vec![user_message(content)],
            ..Default::default()
        };
        let chat = messages_to_chat(Some(&req)).unwrap();
        assert_eq!(chat.messages.len(), 2);
        assert_eq!(chat.messages[0].role, "tool");
        assert_eq!(chat.messages[0].tool_call_id, "toolu_1");
        assert_eq!(chat.messages[0].content, Some(json!("42")));
        assert_eq!(chat.messages[1].role, "user");
    }

    #[test]
    fn tool_use_only_assistant_turn_omits_content() {
        let msg = Message {
            role: "assistant".to_string(),
            content: Some(json!([
                {"type": "text", "text": "thinking out loud"},
                {"type": "thinking", "thinking": "secret"},
                {"type": "tool_use", "id": "toolu_9", "name": "lookup", "input": {"q": "x"}}
            ])),
        };
        let chat = assistant_turn_to_chat(&msg);
        assert_eq!(chat.content, Some(json!("thinking out loud")));
        assert_eq!(chat.tool_calls.len(), 1);
        assert_eq!(chat.tool_calls[0].function.name, "lookup");
        assert_eq!(chat.tool_calls[0].function.arguments, "{\"q\":\"x\"}");

        let only_tools = assistant_turn_to_chat(&Message {
            role: "assistant".to_string(),
            content: Some(json!([
                {"type": "thinking", "thinking": "secret"},
                {"type": "tool_use", "id": "toolu_9", "name": "lookup", "input": {}}
            ])),
        });
        assert_eq!(only_tools.content, None);
    }

    #[test]
    fn invalid_arguments_become_empty_object() {
        assert_eq!(arguments_to_input(""), json!({}));
        assert_eq!(arguments_to_input("not json"), json!({}));
        assert_eq!(arguments_to_input("[1,2]"), json!({}));
        assert_eq!(arguments_to_input("{\"a\":1}"), json!({"a": 1}));
    }

    #[test]
    fn chat_to_messages_maps_reasoning_text_and_tools() {
        let resp = ChatResponse {
            id: "abc".to_string(),
            choices: vec![super::super::ChatChoice {
                index: 0,
                message: ChatMessage {
                    role: "assistant".to_string(),
                    content: Some(json!("hello")),
                    reasoning_content: "because".to_string(),
                    tool_calls: vec![ToolCall {
                        index: None,
                        id: "call_1".to_string(),
                        r#type: "function".to_string(),
                        function: FunctionCall {
                            name: "lookup".to_string(),
                            arguments: "{\"q\":1}".to_string(),
                        },
                    }],
                    ..Default::default()
                },
                finish_reason: FINISH_TOOL_CALLS.to_string(),
            }],
            usage: Some(super::super::ChatUsage {
                prompt_tokens: 3,
                completion_tokens: 4,
                total_tokens: 7,
            }),
            ..Default::default()
        };
        let out = chat_to_messages(&resp, "claude-3");
        assert_eq!(out.id, "msg_abc");
        assert_eq!(out.model, "claude-3");
        assert_eq!(out.content[0].r#type, "thinking");
        assert_eq!(out.content[0].thinking, "because");
        assert_eq!(out.content[1].r#type, "text");
        assert_eq!(out.content[1].text, "hello");
        assert_eq!(out.content[2].r#type, "tool_use");
        assert_eq!(out.content[2].name, "lookup");
        assert_eq!(out.stop_reason, STOP_TOOL_USE);
        assert_eq!(
            out.usage,
            MessagesUsage {
                input_tokens: 3,
                output_tokens: 4,
                ..Default::default()
            }
        );
    }

    #[test]
    fn stop_reason_mapping() {
        assert_eq!(stop_reason_from_finish("stop", false), STOP_END_TURN);
        assert_eq!(stop_reason_from_finish("length", false), STOP_MAX_TOKENS);
        assert_eq!(stop_reason_from_finish("tool_calls", false), STOP_TOOL_USE);
        assert_eq!(stop_reason_from_finish("stop", true), STOP_TOOL_USE);
        assert_eq!(stop_reason_from_finish("", false), STOP_END_TURN);
    }
}
