//! DeepSeek request types and conversation serialization.
//!
//! `vava-core` message types are provider-independent; this module is the
//! explicit DeepSeek serializer that turns them into the wire format:
//!
//! ```text
//! vava-core::Message
//!         │
//!         ▼
//!   DeepSeek serializer      (this module)
//!         │
//!         ▼
//!   DeepSeek API JSON
//! ```
//!
//! No `vava-core` type carries DeepSeek-specific serde annotations.

use serde::Serialize;
use serde_json::Value;

use vava_core::{AssistantMessage, Message, ToolCall, ToolDefinition, ToolResultMessage};

/// A chat completion request for `POST /chat/completions`.
#[derive(Debug, Clone, Serialize)]
pub struct DeepSeekRequest {
    /// The model to query.
    pub model: String,
    /// The conversation, including the leading system message.
    pub messages: Vec<WireMessage>,
    /// Tool definitions advertised to the model; omitted when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<WireTool>,
    /// Whether to stream the response as SSE chunks.
    pub stream: bool,
    /// DeepSeek's `thinking` parameter; omitted when unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<Thinking>,
}

impl DeepSeekRequest {
    /// Build a request from provider-independent conversation types.
    ///
    /// `system` becomes the first message of the request. The only error is
    /// a tool call whose arguments cannot be re-serialized to JSON — which
    /// cannot happen for arguments that were parsed from real JSON.
    pub fn chat(
        model: impl Into<String>,
        system: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
        stream: bool,
        thinking: Option<Thinking>,
    ) -> Result<Self, serde_json::Error> {
        let mut wire_messages = Vec::with_capacity(messages.len() + 1);
        wire_messages.push(WireMessage::system(system));
        for message in messages {
            wire_messages.push(match message {
                Message::User(user) => WireMessage::user(&user.content),
                Message::Assistant(assistant) => WireMessage::assistant(assistant)?,
                Message::ToolResult(tool_result) => WireMessage::tool(tool_result),
            });
        }

        let wire_tools = tools
            .iter()
            .map(WireTool::from_definition)
            .collect::<Vec<_>>();

        Ok(Self {
            model: model.into(),
            messages: wire_messages,
            tools: wire_tools,
            stream,
            thinking,
        })
    }
}

/// The `thinking` parameter, which serializes as `{"type": "enabled"}` or
/// `{"type": "disabled"}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Thinking {
    #[serde(rename = "type")]
    pub mode: ThinkingMode,
}

impl Thinking {
    /// Request thinking mode explicitly.
    pub const ENABLED: Self = Self {
        mode: ThinkingMode::Enabled,
    };
    /// Disable thinking mode explicitly.
    pub const DISABLED: Self = Self {
        mode: ThinkingMode::Disabled,
    };
}

/// Whether the model should think (produce `reasoning_content`) or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingMode {
    Enabled,
    Disabled,
}

/// A message in the wire `messages` array.
#[derive(Debug, Clone, Serialize)]
pub struct WireMessage {
    pub role: Role,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// DeepSeek-specific: the assistant's reasoning text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<WireToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl WireMessage {
    pub fn system(content: &str) -> Self {
        Self {
            role: Role::System,
            content: Some(content.to_string()),
            reasoning_content: None,
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    pub fn user(content: &str) -> Self {
        Self {
            role: Role::User,
            content: Some(content.to_string()),
            reasoning_content: None,
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    /// Serialize an assistant message. Reasoning content is carried through
    /// even when the message contains tool calls, because DeepSeek needs it
    /// back on subsequent turns.
    pub fn assistant(message: &AssistantMessage) -> Result<Self, serde_json::Error> {
        let mut tool_calls = Vec::with_capacity(message.tool_calls.len());
        for call in &message.tool_calls {
            tool_calls.push(WireToolCall::from_core(call)?);
        }
        Ok(Self {
            role: Role::Assistant,
            content: Some(message.content.clone()),
            reasoning_content: message.reasoning_content.clone(),
            tool_calls,
            tool_call_id: None,
        })
    }

    pub fn tool(message: &ToolResultMessage) -> Self {
        Self {
            role: Role::Tool,
            content: Some(message.content.clone()),
            reasoning_content: None,
            tool_calls: Vec::new(),
            tool_call_id: Some(message.tool_call_id.clone()),
        }
    }
}

/// The role of a wire message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// A tool call in an assistant wire message (OpenAI-compatible shape).
#[derive(Debug, Clone, Serialize)]
pub struct WireToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: WireFunctionCall,
}

/// The `function` part of a wire tool call. `arguments` is a JSON-encoded
/// *string*, per the OpenAI-compatible format.
#[derive(Debug, Clone, Serialize)]
pub struct WireFunctionCall {
    pub name: String,
    pub arguments: String,
}

impl WireToolCall {
    pub fn from_core(call: &ToolCall) -> Result<Self, serde_json::Error> {
        Ok(Self {
            id: call.id.clone(),
            kind: "function",
            function: WireFunctionCall {
                name: call.name.clone(),
                arguments: serde_json::to_string(&call.arguments)?,
            },
        })
    }
}

/// A tool definition in the wire `tools` array.
#[derive(Debug, Clone, Serialize)]
pub struct WireTool {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: WireFunction,
}

#[derive(Debug, Clone, Serialize)]
pub struct WireFunction {
    pub name: String,
    pub description: String,
    /// A JSON Schema describing the arguments.
    pub parameters: Value,
}

impl WireTool {
    pub fn from_definition(definition: &ToolDefinition) -> Self {
        Self {
            kind: "function",
            function: WireFunction {
                name: definition.name.clone(),
                description: definition.description.clone(),
                parameters: definition.parameters.clone(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use vava_core::{ToolResultMessage, UserMessage};

    fn messages() -> Vec<Message> {
        vec![Message::User(UserMessage {
            content: "hi".into(),
        })]
    }

    fn request() -> DeepSeekRequest {
        DeepSeekRequest::chat(
            "deepseek-chat",
            "You are vava.",
            &messages(),
            &[],
            true,
            None,
        )
        .unwrap()
    }

    #[test]
    fn minimal_chat_request_serializes() {
        let s = serde_json::to_string(&request()).unwrap();
        assert_eq!(
            s,
            r#"{"model":"deepseek-chat","messages":[{"role":"system","content":"You are vava."},{"role":"user","content":"hi"}],"stream":true}"#
        );
    }

    #[test]
    fn empty_tools_and_thinking_are_omitted() {
        let s = serde_json::to_string(&request()).unwrap();
        assert!(!s.contains("\"tools\""));
        assert!(!s.contains("\"thinking\""));
    }

    #[test]
    fn thinking_parameter_serializes() {
        let enabled = DeepSeekRequest::chat(
            "deepseek-chat",
            "sys",
            &messages(),
            &[],
            true,
            Some(Thinking::ENABLED),
        )
        .unwrap();
        let s = serde_json::to_string(&enabled).unwrap();
        assert!(s.contains(r#""thinking":{"type":"enabled"}"#));

        let disabled = DeepSeekRequest::chat(
            "deepseek-chat",
            "sys",
            &messages(),
            &[],
            true,
            Some(Thinking::DISABLED),
        )
        .unwrap();
        let s = serde_json::to_string(&disabled).unwrap();
        assert!(s.contains(r#""thinking":{"type":"disabled"}"#));
    }

    #[test]
    fn assistant_reasoning_and_tool_calls_serialize() {
        let messages = vec![Message::Assistant(AssistantMessage {
            content: String::new(),
            reasoning_content: Some("let me think".into()),
            tool_calls: vec![ToolCall {
                id: "call_1".into(),
                name: "read".into(),
                arguments: json!({"path": "src/main.rs"}),
            }],
        })];
        let req = DeepSeekRequest::chat("m", "sys", &messages, &[], true, None).unwrap();
        let s = serde_json::to_string(&req).unwrap();
        assert!(
            s.contains(
                r#"{"role":"assistant","content":"","reasoning_content":"let me think","tool_calls":[{"id":"call_1","type":"function","function":{"name":"read","arguments":"{\"path\":\"src/main.rs\"}"}}]}"#
            ),
            "unexpected serialization: {s}"
        );
    }

    #[test]
    fn assistant_without_tool_calls_omits_the_field() {
        let messages = vec![Message::Assistant(AssistantMessage::with_reasoning(
            "done", "thought",
        ))];
        let req = DeepSeekRequest::chat("m", "sys", &messages, &[], true, None).unwrap();
        let s = serde_json::to_string(&req).unwrap();
        assert!(
            s.contains(r#"{"role":"assistant","content":"done","reasoning_content":"thought"}"#)
        );
        assert!(!s.contains("tool_calls"));
    }

    #[test]
    fn tool_result_serializes() {
        let messages = vec![Message::ToolResult(ToolResultMessage {
            tool_call_id: "call_1".into(),
            tool_name: "read".into(),
            content: "hello".into(),
            is_error: false,
        })];
        let req = DeepSeekRequest::chat("m", "sys", &messages, &[], true, None).unwrap();
        let s = serde_json::to_string(&req).unwrap();
        assert!(s.contains(r#"{"role":"tool","content":"hello","tool_call_id":"call_1"}"#));
    }

    #[test]
    fn tools_serialize_with_function_wrapper() {
        let tools = vec![ToolDefinition::new(
            "read",
            "Read a file within the repository.",
            json!({"type": "object"}),
        )];
        let req = DeepSeekRequest::chat("m", "sys", &messages(), &tools, true, None).unwrap();
        let s = serde_json::to_string(&req).unwrap();
        assert!(
            s.contains(
                r#"{"type":"function","function":{"name":"read","description":"Read a file within the repository.","parameters":{"type":"object"}}}"#
            ),
            "unexpected serialization: {s}"
        );
    }

    #[test]
    fn tool_call_arguments_are_json_strings_on_the_wire() {
        let call = ToolCall {
            id: "c".into(),
            name: "edit".into(),
            arguments: json!({"path": "a", "old_text": "x", "new_text": "y"}),
        };
        let wire = WireToolCall::from_core(&call).unwrap();
        // The arguments must be a string that itself parses as JSON.
        let parsed: Value = serde_json::from_str(&wire.function.arguments).unwrap();
        assert_eq!(parsed["old_text"], "x");
        assert_eq!(parsed["new_text"], "y");
    }
}
