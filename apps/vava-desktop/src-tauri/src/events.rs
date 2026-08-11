//! The Rust → React event serialization boundary.
//!
//! `vava-core`'s [`AgentEvent`]s are the frontend-neutral contract between
//! the harness and any UI. This module translates them into the JSON events
//! streamed to React through Tauri channels. Nothing here may leak into
//! `vava-core`; this is the single, explicit boundary.
//!
//! Event payloads use camelCase on the wire (matching the frontend's
//! `DesktopAgentEvent` type); transcript *messages* embedded in events
//! (`assistant_message_completed`) keep the snake_case persistence shape
//! from [`crate::model::DesktopMessage`].

use serde::Serialize;

use vava_core::{AgentEvent, ToolResult};

use crate::model::DesktopMessage;

/// One event streamed from the agent harness to React during a turn.
///
/// Field shapes follow the frontend `DesktopAgentEvent` union:
/// `turn_started`, `text_delta`, `reasoning_delta`, `tool_call_started`,
/// `tool_call_finished`, `assistant_message_completed`, `turn_completed`,
/// `error`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DesktopAgentEvent {
    /// The agent started a new model turn.
    TurnStarted,
    /// A chunk of final-answer text.
    TextDelta { delta: String },
    /// A chunk of reasoning text.
    ReasoningDelta { delta: String },
    /// The model requested a tool call. Arguments may still be streaming,
    /// so `input` may be null or partial.
    ToolCallStarted {
        #[serde(rename = "callId")]
        call_id: String,
        tool: String,
        input: serde_json::Value,
    },
    /// A tool call finished executing.
    ToolCallFinished {
        #[serde(rename = "callId")]
        call_id: String,
        result: DesktopToolResult,
    },
    /// A complete assistant message was assembled and stored.
    AssistantMessageCompleted { message: DesktopMessage },
    /// The agent produced its final response and the turn is over.
    TurnCompleted,
    /// An unrecoverable error occurred; the turn is over.
    Error { message: String },
}

/// The outcome of one tool call, as streamed to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct DesktopToolResult {
    pub content: String,
    #[serde(rename = "isError")]
    pub is_error: bool,
}

impl From<&ToolResult> for DesktopToolResult {
    fn from(result: &ToolResult) -> Self {
        Self {
            content: result.content.clone(),
            is_error: result.is_error,
        }
    }
}

/// The explicit translation from harness events to desktop IPC events.
impl From<&AgentEvent> for DesktopAgentEvent {
    fn from(event: &AgentEvent) -> Self {
        match event {
            AgentEvent::TurnStarted => DesktopAgentEvent::TurnStarted,
            AgentEvent::TextDelta { delta } => DesktopAgentEvent::TextDelta {
                delta: delta.clone(),
            },
            AgentEvent::ReasoningDelta { delta } => DesktopAgentEvent::ReasoningDelta {
                delta: delta.clone(),
            },
            AgentEvent::ToolCallStarted { call } => DesktopAgentEvent::ToolCallStarted {
                call_id: call.id.clone(),
                tool: call.name.clone(),
                input: call.arguments.clone(),
            },
            AgentEvent::ToolCallFinished { call_id, result } => {
                DesktopAgentEvent::ToolCallFinished {
                    call_id: call_id.clone(),
                    result: DesktopToolResult::from(result),
                }
            }
            AgentEvent::AssistantMessageCompleted { message } => {
                DesktopAgentEvent::AssistantMessageCompleted {
                    message: DesktopMessage::from_assistant(message),
                }
            }
            AgentEvent::TurnCompleted => DesktopAgentEvent::TurnCompleted,
            AgentEvent::Error { message } => DesktopAgentEvent::Error {
                message: message.clone(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use vava_core::{AssistantMessage, ToolCall};

    #[test]
    fn text_delta_serializes_with_the_type_tag() {
        let event = DesktopAgentEvent::from(&AgentEvent::TextDelta {
            delta: "hello".into(),
        });
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "text_delta");
        assert_eq!(json["delta"], "hello");
    }

    #[test]
    fn reasoning_delta_serializes() {
        let event = DesktopAgentEvent::from(&AgentEvent::ReasoningDelta {
            delta: "think".into(),
        });
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "reasoning_delta");
        assert_eq!(json["delta"], "think");
    }

    #[test]
    fn tool_call_started_uses_camel_case_payload() {
        let event = DesktopAgentEvent::from(&AgentEvent::ToolCallStarted {
            call: ToolCall {
                id: "call_1".into(),
                name: "read".into(),
                arguments: json!({ "path": "src/main.rs" }),
            },
        });
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "tool_call_started");
        assert_eq!(json["callId"], "call_1");
        assert_eq!(json["tool"], "read");
        assert_eq!(json["input"]["path"], "src/main.rs");
    }

    #[test]
    fn tool_call_finished_serializes_the_result() {
        let event = DesktopAgentEvent::from(&AgentEvent::ToolCallFinished {
            call_id: "call_1".into(),
            result: ToolResult::error("exit code 101"),
        });
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "tool_call_finished");
        assert_eq!(json["callId"], "call_1");
        assert_eq!(json["result"]["content"], "exit code 101");
        assert_eq!(json["result"]["isError"], true);
    }

    #[test]
    fn completed_assistant_message_carries_the_full_message() {
        let message = AssistantMessage {
            content: "done".into(),
            reasoning_content: Some("thought".into()),
            tool_calls: vec![ToolCall::new("c1", "read")],
        };
        let event = DesktopAgentEvent::from(&AgentEvent::AssistantMessageCompleted {
            message: message.clone(),
        });
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "assistant_message_completed");
        assert_eq!(json["message"]["type"], "assistant");
        assert_eq!(json["message"]["content"], "done");
        assert_eq!(json["message"]["reasoning_content"], "thought");
        assert_eq!(json["message"]["tool_calls"][0]["name"], "read");
    }

    #[test]
    fn lifecycle_events_serialize() {
        assert_eq!(
            serde_json::to_value(DesktopAgentEvent::from(&AgentEvent::TurnStarted)).unwrap()["type"],
            "turn_started"
        );
        assert_eq!(
            serde_json::to_value(DesktopAgentEvent::from(&AgentEvent::TurnCompleted)).unwrap()["type"],
            "turn_completed"
        );
        let error = DesktopAgentEvent::from(&AgentEvent::Error {
            message: "kaput".into(),
        });
        let json = serde_json::to_value(&error).unwrap();
        assert_eq!(json["type"], "error");
        assert_eq!(json["message"], "kaput");
    }

    #[test]
    fn every_agent_event_variant_translates() {
        let events = [
            AgentEvent::TurnStarted,
            AgentEvent::TextDelta { delta: "x".into() },
            AgentEvent::ReasoningDelta { delta: "x".into() },
            AgentEvent::ToolCallStarted {
                call: ToolCall::new("c", "read"),
            },
            AgentEvent::ToolCallFinished {
                call_id: "c".into(),
                result: ToolResult::ok("out"),
            },
            AgentEvent::AssistantMessageCompleted {
                message: AssistantMessage::new("x"),
            },
            AgentEvent::TurnCompleted,
            AgentEvent::Error {
                message: "x".into(),
            },
        ];
        for event in &events {
            let desktop = DesktopAgentEvent::from(event);
            assert!(serde_json::to_value(&desktop).is_ok());
        }
    }
}
