//! IPC data-transfer types: the serialization boundary between the Rust
//! backend and React. Field names are camelCase on the wire.
//!
//! These are deliberately *views* of `vava-coding` types, not the types
//! themselves: the frontend receives only what it needs to render, and the
//! domain types stay free of Tauri/serde concerns beyond persistence.

use serde::Serialize;

/// The information React needs after opening a repository.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryInfo {
    /// A stable id for the repository (the session-scoping hash).
    pub id: String,
    /// The repository directory's name, e.g. `brisk-core`.
    pub name: String,
    /// The canonical workspace root path.
    pub root: String,
    /// The active session's id, if a session is initialized.
    pub active_session_id: Option<String>,
    /// The repository's saved sessions, newest first.
    pub sessions: Vec<SessionInfo>,
}

/// One saved session, as shown in the session sidebar.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: String,
    /// RFC 3339 timestamp.
    pub created_at: String,
    /// RFC 3339 timestamp.
    pub updated_at: String,
    /// The first user prompt, if the log contains one.
    pub first_user_message: Option<String>,
}

/// One recent repository, as shown on the launcher screen.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentRepository {
    /// The canonical repository root path.
    pub path: String,
    /// The repository directory's name.
    pub name: String,
    /// RFC 3339 timestamp of the last open.
    pub last_opened_at: String,
    /// Whether the directory still exists on disk.
    pub exists: bool,
}

/// A loaded session: its summary plus the restored transcript.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub session: SessionInfo,
    pub messages: Vec<DesktopMessage>,
}

/// One transcript message, as rendered by React.
///
/// The wire shape mirrors `vava-core`'s persistence format (tagged
/// `type`, snake_case fields), so the original textual session data is
/// preserved exactly; only the camelCase tool-call arguments differ.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DesktopMessage {
    /// A prompt or instruction from the user.
    User { content: String },
    /// A complete model response: text, optional reasoning, tool calls.
    Assistant {
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        reasoning_content: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<DesktopToolCall>,
    },
    /// The result of executing one tool call.
    #[serde(rename = "tool")]
    ToolResult {
        tool_call_id: String,
        tool_name: String,
        content: String,
        is_error: bool,
    },
}

/// One tool call within an assistant message.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// The explicit conversion boundary from the domain transcript to the IPC
/// shape. This is the only place `vava-core` messages cross into the
/// frontend view layer.
impl From<&vava_core::Message> for DesktopMessage {
    fn from(message: &vava_core::Message) -> Self {
        use vava_core::Message as M;
        match message {
            M::User(user) => DesktopMessage::User {
                content: user.content.clone(),
            },
            M::Assistant(assistant) => DesktopMessage::from_assistant(assistant),
            M::ToolResult(result) => DesktopMessage::ToolResult {
                tool_call_id: result.tool_call_id.clone(),
                tool_name: result.tool_name.clone(),
                content: result.content.clone(),
                is_error: result.is_error,
            },
        }
    }
}

impl DesktopMessage {
    /// Convert a complete assistant message (used by the event boundary,
    /// which receives `AssistantMessage` directly rather than `Message`).
    pub fn from_assistant(assistant: &vava_core::AssistantMessage) -> Self {
        DesktopMessage::Assistant {
            content: assistant.content.clone(),
            reasoning_content: assistant.reasoning_content.clone(),
            tool_calls: assistant
                .tool_calls
                .iter()
                .map(|call| DesktopToolCall {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    arguments: call.arguments.clone(),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_info_serializes_with_camel_case_fields() {
        let info = RepositoryInfo {
            id: "abc123".into(),
            name: "brisk-core".into(),
            root: "/repo/brisk-core".into(),
            active_session_id: Some("s1".into()),
            sessions: vec![SessionInfo {
                id: "s1".into(),
                created_at: "2026-08-11T12:00:00Z".into(),
                updated_at: "2026-08-11T13:00:00Z".into(),
                first_user_message: Some("fix tests".into()),
            }],
        };
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["name"], "brisk-core");
        assert_eq!(json["activeSessionId"], "s1");
        assert_eq!(json["sessions"][0]["firstUserMessage"], "fix tests");
        // The wire shape is camelCase; snake_case keys must not leak.
        assert!(json.get("active_session_id").is_none());
    }

    #[test]
    fn optional_fields_serialize_as_null() {
        let info = RepositoryInfo {
            id: "abc".into(),
            name: "r".into(),
            root: "/r".into(),
            active_session_id: None,
            sessions: Vec::new(),
        };
        let json = serde_json::to_value(&info).unwrap();
        assert!(json["activeSessionId"].is_null());
        assert!(json["sessions"].is_array());
        assert!(json["sessions"].as_array().unwrap().is_empty());
    }

    #[test]
    fn recent_repository_serializes_marks_missing() {
        let recent = RecentRepository {
            path: "/gone".into(),
            name: "gone".into(),
            last_opened_at: "2026-08-11T12:00:00Z".into(),
            exists: false,
        };
        let json = serde_json::to_value(&recent).unwrap();
        assert_eq!(json["exists"], false);
        assert_eq!(json["lastOpenedAt"], "2026-08-11T12:00:00Z");
    }

    #[test]
    fn user_message_converts_and_serializes() {
        use vava_core::{Message, UserMessage};
        let desktop: DesktopMessage = DesktopMessage::from(&Message::User(UserMessage {
            content: "fix the tests".into(),
        }));
        let json = serde_json::to_value(&desktop).unwrap();
        assert_eq!(json["type"], "user");
        assert_eq!(json["content"], "fix the tests");
    }

    #[test]
    fn assistant_message_keeps_reasoning_and_tool_calls() {
        use serde_json::json;
        use vava_core::{AssistantMessage, Message, ToolCall};
        let message = Message::Assistant(AssistantMessage {
            content: "let me check".into(),
            reasoning_content: Some("thinking…".into()),
            tool_calls: vec![ToolCall {
                id: "call_1".into(),
                name: "read".into(),
                arguments: json!({ "path": "src/main.rs" }),
            }],
        });
        let desktop: DesktopMessage = DesktopMessage::from(&message);
        let json = serde_json::to_value(&desktop).unwrap();
        assert_eq!(json["type"], "assistant");
        assert_eq!(json["content"], "let me check");
        // The transcript mirrors the persistence format (snake_case fields).
        assert_eq!(json["reasoning_content"], "thinking…");
        assert_eq!(json["tool_calls"][0]["name"], "read");
        assert_eq!(json["tool_calls"][0]["arguments"]["path"], "src/main.rs");
        // The persisted tool-call shape stays intact.
        assert_eq!(json["tool_calls"][0]["id"], "call_1");
    }

    #[test]
    fn assistant_without_reasoning_or_calls_stays_lean() {
        use vava_core::{AssistantMessage, Message};
        let desktop: DesktopMessage =
            DesktopMessage::from(&Message::Assistant(AssistantMessage::new("done")));
        let json = serde_json::to_value(&desktop).unwrap();
        assert_eq!(json["type"], "assistant");
        assert_eq!(json["content"], "done");
        assert!(json.get("reasoningContent").is_none());
        assert!(json.get("toolCalls").is_none());
    }

    #[test]
    fn tool_result_uses_the_persistence_tag() {
        use vava_core::{Message, ToolResultMessage};
        let desktop: DesktopMessage =
            DesktopMessage::from(&Message::ToolResult(ToolResultMessage {
                tool_call_id: "call_1".into(),
                tool_name: "bash".into(),
                content: "exit code 101".into(),
                is_error: true,
            }));
        let json = serde_json::to_value(&desktop).unwrap();
        assert_eq!(json["type"], "tool");
        assert_eq!(json["tool_call_id"], "call_1");
        assert_eq!(json["tool_name"], "bash");
        assert_eq!(json["is_error"], true);
    }

    #[test]
    fn session_view_serializes_with_session_and_messages() {
        use vava_core::{Message, UserMessage};
        let view = SessionView {
            session: SessionInfo {
                id: "s1".into(),
                created_at: "2026-08-11T12:00:00Z".into(),
                updated_at: "2026-08-11T13:00:00Z".into(),
                first_user_message: Some("fix tests".into()),
            },
            messages: vec![DesktopMessage::from(&Message::User(UserMessage {
                content: "fix tests".into(),
            }))],
        };
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["session"]["id"], "s1");
        assert_eq!(json["session"]["firstUserMessage"], "fix tests");
        assert_eq!(json["messages"][0]["type"], "user");
        assert_eq!(json["messages"][0]["content"], "fix tests");
    }
}
