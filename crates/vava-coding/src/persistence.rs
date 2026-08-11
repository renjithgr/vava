//! Append-only JSONL session persistence.
//!
//! Every session is one `.jsonl` file under `<data_dir>/vava/sessions/`.
//! Records look like:
//!
//! ```json
//! {"type":"session","id":"...","cwd":"/projects/foo"}
//! {"type":"user","content":"Fix the tests"}
//! {"type":"assistant","content":"","reasoning_content":"...","tool_calls":[...]}
//! {"type":"tool","tool_call_id":"...","tool_name":"bash","content":"...","is_error":false}
//! ```
//!
//! The message records reuse `vava-core`'s serde representation, so
//! `reasoning_content` survives by construction. Replay is tolerant of a
//! partially written final line (a crash mid-write).

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use vava_core::Message;

/// The first record of a session log. The `"type":"session"` tag is
/// written but not modeled (serde ignores unknown fields on read).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionHeader {
    pub id: String,
    pub cwd: String,
}

impl SessionHeader {
    fn to_record(&self) -> String {
        serde_json::json!({"type": "session", "id": self.id, "cwd": self.cwd}).to_string()
    }
}

/// Where session logs live: `<data_dir>/vava/sessions/<id>.jsonl`.
#[derive(Debug, Clone)]
pub struct SessionStore {
    dir: PathBuf,
}

impl SessionStore {
    /// The platform data directory: `~/.local/share/vava/sessions` on
    /// Linux, `~/Library/Application Support/vava/sessions` on macOS.
    pub fn open() -> Result<Self, PersistError> {
        let dir = dirs::data_local_dir()
            .map(|base| base.join("vava").join("sessions"))
            .ok_or(PersistError::NoDataDir)?;
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    /// A store rooted at an explicit directory (used by tests).
    pub fn open_at(dir: PathBuf) -> Result<Self, PersistError> {
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Create a new session log and write its header.
    pub fn create(&self, cwd: &Path) -> Result<SessionLog, PersistError> {
        let id = Uuid::new_v4().to_string();
        let log = SessionLog {
            id: id.clone(),
            path: self.dir.join(format!("{id}.jsonl")),
        };
        let header = SessionHeader {
            id,
            cwd: cwd.display().to_string(),
        };
        let mut file = OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(&log.path)?;
        writeln!(file, "{}", header.to_record())?;
        file.flush()?;
        Ok(log)
    }

    /// Replay a session's transcript from its log.
    ///
    /// Tolerates a partially written final line (a crash mid-write); any
    /// other unreadable record is a corruption error.
    pub fn open_session(&self, id: &str) -> Result<(SessionHeader, Vec<Message>), PersistError> {
        let path = self.dir.join(format!("{id}.jsonl"));
        let content = std::fs::read_to_string(&path)
            .map_err(|_| PersistError::NotFound { id: id.to_string() })?;

        let mut lines = content.lines();
        let header_line = lines.next().ok_or_else(|| PersistError::Corrupt {
            id: id.to_string(),
            message: "session log is empty".into(),
        })?;
        let header: SessionHeader =
            serde_json::from_str(header_line).map_err(|error| PersistError::Corrupt {
                id: id.to_string(),
                message: format!("bad header: {error}"),
            })?;

        let mut messages = Vec::new();
        for (index, line) in lines.enumerate() {
            match serde_json::from_str::<Message>(line) {
                Ok(message) => messages.push(message),
                Err(_) if is_last_line(&content, line) => break,
                Err(error) => {
                    return Err(PersistError::Corrupt {
                        id: id.to_string(),
                        message: format!("record {}: {error}", index + 2),
                    });
                }
            }
        }
        Ok((header, messages))
    }

    /// List all sessions (header records), sorted by id.
    pub fn list(&self) -> Result<Vec<SessionHeader>, PersistError> {
        let mut headers = Vec::new();
        for entry in std::fs::read_dir(&self.dir)? {
            let entry = entry?;
            if entry.path().extension().is_some_and(|ext| ext == "jsonl")
                && let Ok(content) = std::fs::read_to_string(entry.path())
                && let Some(line) = content.lines().next()
                && let Ok(header) = serde_json::from_str::<SessionHeader>(line)
            {
                headers.push(header);
            }
        }
        headers.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(headers)
    }
}

/// A single session's log: metadata plus append access.
#[derive(Debug, Clone)]
pub struct SessionLog {
    id: String,
    path: PathBuf,
}

impl SessionLog {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one message record to this log.
    pub fn append(&self, message: &Message) -> Result<(), PersistError> {
        append_log(&self.path, message)
    }
}

/// Append one message record to the log at `path`. The file is opened in
/// append mode, so a crashed process never loses previously written
/// records.
pub fn append_log(path: &Path, message: &Message) -> Result<(), PersistError> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let line = serde_json::to_string(message)?;
    writeln!(file, "{line}")?;
    file.flush()?;
    Ok(())
}

/// Whether `line` is the final line of `content` (used to tolerate a
/// truncated final record).
fn is_last_line(content: &str, line: &str) -> bool {
    content.lines().last() == Some(line)
}

/// Errors from session persistence.
#[derive(Debug, Error)]
pub enum PersistError {
    /// No platform data directory could be determined.
    #[error("no platform data directory available")]
    NoDataDir,
    /// A filesystem operation on the session directory or log failed.
    #[error("session file error: {0}")]
    Io(#[from] std::io::Error),
    /// A record could not be serialized.
    #[error("could not serialize session record: {0}")]
    Serialize(#[from] serde_json::Error),
    /// No log exists for the requested session id.
    #[error("session `{id}` not found")]
    NotFound { id: String },
    /// The log exists but cannot be replayed.
    #[error("session `{id}` is corrupt: {message}")]
    Corrupt { id: String, message: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use vava_core::{AssistantMessage, ToolCall, ToolResultMessage, UserMessage};

    use crate::tools::test_util::TestDir;

    fn store() -> (SessionStore, TestDir) {
        let dir = TestDir::new();
        (
            SessionStore::open_at(dir.path().to_path_buf()).unwrap(),
            dir,
        )
    }

    fn user(content: &str) -> Message {
        Message::User(UserMessage {
            content: content.into(),
        })
    }

    fn assistant_with_reasoning_and_call() -> Message {
        Message::Assistant(AssistantMessage {
            content: String::new(),
            reasoning_content: Some("let me think".into()),
            tool_calls: vec![ToolCall {
                id: "call_1".into(),
                name: "bash".into(),
                arguments: json!({"command": "cargo test"}),
            }],
        })
    }

    #[test]
    fn create_writes_the_header_record() {
        let (store, _dir) = store();
        let log = store.create(&PathBuf::from("/projects/foo")).unwrap();
        let content = std::fs::read_to_string(log.path()).unwrap();
        // Key order is not part of the contract; parse and compare fields.
        let record: serde_json::Value =
            serde_json::from_str(content.lines().next().unwrap()).unwrap();
        assert_eq!(record["type"], "session");
        assert_eq!(record["id"], log.id());
        assert_eq!(record["cwd"], "/projects/foo");
    }

    #[test]
    fn append_and_replay_round_trip() {
        let (store, _dir) = store();
        let log = store.create(&PathBuf::from("/p")).unwrap();
        log.append(&user("fix the tests")).unwrap();
        log.append(&assistant_with_reasoning_and_call()).unwrap();
        log.append(&Message::ToolResult(ToolResultMessage {
            tool_call_id: "call_1".into(),
            tool_name: "bash".into(),
            content: "exit code 101".into(),
            is_error: true,
        }))
        .unwrap();

        let (header, messages) = store.open_session(log.id()).unwrap();
        assert_eq!(header.cwd, "/p");
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0], user("fix the tests"));
        assert_eq!(messages[1], assistant_with_reasoning_and_call());
        let Message::ToolResult(result) = &messages[2] else {
            panic!("expected tool result");
        };
        assert!(result.is_error);
        assert_eq!(result.content, "exit code 101");
    }

    #[test]
    fn reasoning_survives_persistence() {
        let (store, _dir) = store();
        let log = store.create(&PathBuf::from("/p")).unwrap();
        log.append(&assistant_with_reasoning_and_call()).unwrap();

        let (_, messages) = store.open_session(log.id()).unwrap();
        let Message::Assistant(assistant) = &messages[0] else {
            panic!("expected assistant message");
        };
        assert_eq!(assistant.reasoning_content.as_deref(), Some("let me think"));
        assert_eq!(assistant.tool_calls.len(), 1);
    }

    #[test]
    fn tolerates_a_truncated_final_line() {
        let (store, _dir) = store();
        let log = store.create(&PathBuf::from("/p")).unwrap();
        log.append(&user("first")).unwrap();
        // Simulate a crash mid-write: append a partial JSON line.
        let mut file = OpenOptions::new().append(true).open(log.path()).unwrap();
        writeln!(file, r#"{{"type":"user","content":"trun"#).unwrap();
        drop(file);

        let (_, messages) = store.open_session(log.id()).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0], user("first"));
    }

    #[test]
    fn corrupt_middle_records_are_errors() {
        let (store, _dir) = store();
        let log = store.create(&PathBuf::from("/p")).unwrap();
        log.append(&user("first")).unwrap();
        let mut file = OpenOptions::new().append(true).open(log.path()).unwrap();
        writeln!(file, "this is not json").unwrap();
        writeln!(file, "{}", serde_json::to_string(&user("second")).unwrap()).unwrap();
        drop(file);

        let err = store.open_session(log.id()).unwrap_err();
        assert!(matches!(err, PersistError::Corrupt { .. }));
    }

    #[test]
    fn missing_session_is_not_found() {
        let (store, _dir) = store();
        let err = store.open_session("does-not-exist").unwrap_err();
        assert!(matches!(err, PersistError::NotFound { .. }));
    }

    #[test]
    fn list_returns_all_sessions() {
        let (store, _dir) = store();
        store.create(&PathBuf::from("/a")).unwrap();
        store.create(&PathBuf::from("/b")).unwrap();
        let headers = store.list().unwrap();
        assert_eq!(headers.len(), 2);
    }

    #[test]
    fn replay_through_the_core_serializer_matches_the_persistence_format() {
        // The JSONL records are exactly vava-core's serde representation.
        let (store, _dir) = store();
        let log = store.create(&PathBuf::from("/p")).unwrap();
        log.append(&assistant_with_reasoning_and_call()).unwrap();
        let line = std::fs::read_to_string(log.path())
            .unwrap()
            .lines()
            .nth(1)
            .unwrap()
            .to_string();
        assert_eq!(
            line,
            r#"{"type":"assistant","content":"","reasoning_content":"let me think","tool_calls":[{"id":"call_1","name":"bash","arguments":{"command":"cargo test"}}]}"#
        );
    }
}
