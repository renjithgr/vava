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
}
