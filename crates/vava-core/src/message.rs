//! Conversation message types.
//!
//! These types describe a conversation transcript, not a wire protocol.
//! They are provider-independent: [`Message`] is what the agent loop stores,
//! persists, and replays. `vava-deepseek` is responsible for serializing
//! these into the DeepSeek API format, and for parsing DeepSeek responses
//! back into them.
//!
//! The serde representation here is the *persistence* format used by the
//! JSONL session log (`{"type":"user"|"assistant"|"tool", ...}`), so the
//! shape is stable and easy to read by hand.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tool::ToolResult;

/// One message in the conversation transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    /// A prompt or instruction from the user.
    User(UserMessage),
    /// A complete response produced by the model, which may contain text,
    /// reasoning, tool calls, or any combination.
    Assistant(AssistantMessage),
    /// The result of executing one tool call.
    #[serde(rename = "tool")]
    ToolResult(ToolResultMessage),
}

/// A message from the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserMessage {
    /// The user's prompt text.
    pub content: String,
}

/// A complete assistant message.
///
/// DeepSeek may emit `reasoning_content` (the model's chain of thought)
/// alongside `content` and tool calls. Reasoning is a first-class part of the
/// message: it must survive streaming, tool-call loops, persistence, and
/// restoration. Do not discard it when a message contains tool calls.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssistantMessage {
    /// The final-answer text produced so far (empty while tool calls stream).
    pub content: String,
    /// The model's reasoning text, if the model emitted any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    /// Tool calls requested by the model, if any.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
}

impl AssistantMessage {
    /// A plain assistant message with no reasoning and no tool calls.
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            reasoning_content: None,
            tool_calls: Vec::new(),
        }
    }

    /// An assistant message with reasoning text attached.
    pub fn with_reasoning(content: impl Into<String>, reasoning: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            reasoning_content: Some(reasoning.into()),
            tool_calls: Vec::new(),
        }
    }

    /// Whether this message ends the turn — i.e. it has no tool calls to run.
    pub fn is_final(&self) -> bool {
        self.tool_calls.is_empty()
    }
}

/// A tool call requested by the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// The call id, echoed back when the tool result is returned to the model.
    pub id: String,
    /// The name of the tool to invoke.
    pub name: String,
    /// The arguments as a JSON value. While streaming, these are assembled
    /// fragment by fragment before being parsed.
    #[serde(default)]
    pub arguments: Value,
}

impl ToolCall {
    /// A tool call with no arguments (used while a call is still streaming).
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments: Value::Null,
        }
    }
}

/// The result of one tool call, stored in the transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResultMessage {
    /// The id of the [`ToolCall`] this result answers.
    pub tool_call_id: String,
    /// The name of the tool that was executed.
    pub tool_name: String,
    /// The tool's output, formatted as text.
    pub content: String,
    /// Whether the tool failed. Errors are returned to the model as content
    /// rather than aborting the conversation.
    #[serde(default)]
    pub is_error: bool,
}

impl ToolResultMessage {
    /// Build the transcript message for a call and its execution result.
    pub fn from_call(call: &ToolCall, result: ToolResult) -> Self {
        Self {
            tool_call_id: call.id.clone(),
            tool_name: call.name.clone(),
            content: result.content,
            is_error: result.is_error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The exact persistence shapes are part of the contract: the session
    /// log is append-only JSONL, so old files must keep replaying.
    #[test]
    fn user_serializes_with_type_tag() {
        let m = Message::User(UserMessage {
            content: "explain this repo".into(),
        });
        let s = serde_json::to_string(&m).unwrap();
        assert_eq!(s, r#"{"type":"user","content":"explain this repo"}"#);
    }

    #[test]
    fn plain_assistant_omits_reasoning_and_tool_calls() {
        let m = Message::Assistant(AssistantMessage::new("hello"));
        let s = serde_json::to_string(&m).unwrap();
        assert_eq!(s, r#"{"type":"assistant","content":"hello"}"#);
    }

    #[test]
    fn assistant_round_trips_reasoning_and_tool_calls() {
        let original = Message::Assistant(AssistantMessage {
            content: String::new(),
            reasoning_content: Some("let me think".into()),
            tool_calls: vec![ToolCall {
                id: "call_1".into(),
                name: "read".into(),
                arguments: json!({ "path": "src/main.rs" }),
            }],
        });
        let s = serde_json::to_string(&original).unwrap();
        assert_eq!(
            s,
            r#"{"type":"assistant","content":"","reasoning_content":"let me think","tool_calls":[{"id":"call_1","name":"read","arguments":{"path":"src/main.rs"}}]}"#
        );

        let back: Message = serde_json::from_str(&s).unwrap();
        assert_eq!(back, original);
        // Reasoning must survive the round trip even in the presence of tool calls.
        let Message::Assistant(a) = back else {
            panic!("expected assistant message");
        };
        assert_eq!(a.reasoning_content.as_deref(), Some("let me think"));
    }

    #[test]
    fn multi_key_arguments_round_trip_regardless_of_key_order() {
        // serde_json sorts object keys unless the `preserve_order` feature is
        // enabled, so key order is not part of the persistence contract.
        let original = Message::Assistant(AssistantMessage {
            content: String::new(),
            reasoning_content: None,
            tool_calls: vec![ToolCall {
                id: "c".into(),
                name: "read".into(),
                arguments: json!({ "path": "a", "offset": 3, "nested": { "z": 1, "a": 2 } }),
            }],
        });
        let s = serde_json::to_string(&original).unwrap();
        let back: Message = serde_json::from_str(&s).unwrap();
        assert_eq!(back, original);
    }

    #[test]
    fn tool_call_arguments_accept_arbitrary_json() {
        let call = ToolCall {
            id: "c".into(),
            name: "bash".into(),
            arguments: json!({"command": "echo hi", "nested": {"a": [1, 2, 3]}}),
        };
        let s = serde_json::to_string(&Message::ToolResult(ToolResultMessage::from_call(
            &call,
            ToolResult::ok("hi"),
        )))
        .unwrap();
        assert_eq!(
            s,
            r#"{"type":"tool","tool_call_id":"c","tool_name":"bash","content":"hi","is_error":false}"#
        );
    }

    #[test]
    fn deserialization_tolerates_missing_optional_fields() {
        // Older logs may lack reasoning_content / tool_calls / is_error.
        let back: Message = serde_json::from_str(r#"{"type":"assistant","content":"x"}"#).unwrap();
        let Message::Assistant(a) = back else {
            panic!("expected assistant message");
        };
        assert_eq!(a.content, "x");
        assert_eq!(a.reasoning_content, None);
        assert!(a.tool_calls.is_empty());

        let back: Message = serde_json::from_str(
            r#"{"type":"tool","tool_call_id":"c","tool_name":"read","content":"o"}"#,
        )
        .unwrap();
        let Message::ToolResult(t) = back else {
            panic!("expected tool message");
        };
        assert!(!t.is_error);
    }

    #[test]
    fn tool_result_from_call_carries_result() {
        let call = ToolCall {
            id: "call_9".into(),
            name: "bash".into(),
            arguments: json!({"command": "cargo test"}),
        };
        let result = ToolResult::error("exit code 101");
        let m = ToolResultMessage::from_call(&call, result);
        assert_eq!(m.tool_call_id, "call_9");
        assert_eq!(m.tool_name, "bash");
        assert_eq!(m.content, "exit code 101");
        assert!(m.is_error);
    }

    #[test]
    fn assistant_is_final_only_without_tool_calls() {
        assert!(AssistantMessage::new("done").is_final());
        let with_call = AssistantMessage {
            content: String::new(),
            reasoning_content: None,
            tool_calls: vec![ToolCall::new("c", "read")],
        };
        assert!(!with_call.is_final());
    }

    #[test]
    fn tool_call_new_has_null_arguments() {
        let call = ToolCall::new("c1", "read");
        assert_eq!(call.id, "c1");
        assert_eq!(call.name, "read");
        assert_eq!(call.arguments, Value::Null);
    }
}
