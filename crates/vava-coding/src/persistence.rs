//! Append-only JSONL session persistence.
//!
//! Every session is one `.jsonl` file under `<data_dir>/vava/sessions/`.
//! Sessions are scoped to a repository: a stable hash of the canonicalized
//! repository root selects a subdirectory, and every session of that
//! repository lives in it:
//!
//! ```text
//! ~/.local/share/vava/sessions/
//!     <repo-hash>/
//!         01KABC....jsonl
//!         01KDEF....jsonl
//! ```
//!
//! Records look like:
//!
//! ```json
//! {"type":"session","id":"...","cwd":"/projects/foo","created_at":"..."}
//! {"type":"user","content":"Fix the tests"}
//! {"type":"assistant","content":"","reasoning_content":"...","tool_calls":[...]}
//! {"type":"tool","tool_call_id":"...","tool_name":"bash","content":"...","is_error":false}
//! ```
//!
//! The message records reuse `vava-core`'s serde representation, so
//! `reasoning_content` survives by construction. Replay is tolerant of a
//! partially written final line (a crash mid-write).
//!
//! Sessions written before repository scoping (flat files directly under
//! `sessions/`) are still found: their header carries the original `cwd`,
//! which is matched against the requested repository root. They are never
//! rewritten during loading.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use vava_core::Message;

/// A typed session identifier.
///
/// Session ids are opaque strings (currently UUIDs). All display and
/// matching logic lives here so the CLI never slices raw strings.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SessionId(String);

impl SessionId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The full id, as stored on disk.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The full id (same as [`SessionId::as_str`]; explicit for callers
    /// that want to emphasize they are displaying the full value).
    pub fn full(&self) -> &str {
        &self.0
    }

    /// A short display form (the first few characters), enough to tell
    /// sessions apart in a picker.
    pub fn short(&self) -> &str {
        self.0.get(..6).unwrap_or(&self.0)
    }

    /// Whether this id starts with `prefix`.
    pub fn matches_prefix(&self, prefix: &str) -> bool {
        self.0.starts_with(prefix)
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for SessionId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// The first record of a session log. The `"type":"session"` tag is
/// written but not modeled (serde ignores unknown fields on read).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionHeader {
    pub id: String,
    pub cwd: String,
    /// When the session was created, as an RFC 3339 string. Older logs
    /// lack this field; readers fall back to the file's modification time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}

impl SessionHeader {
    fn to_record(&self) -> String {
        serde_json::json!({
            "type": "session",
            "id": self.id,
            "cwd": self.cwd,
            "created_at": self.created_at,
        })
        .to_string()
    }
}

/// What a frontend needs to display one session in a list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSummary {
    pub id: SessionId,
    pub repository_root: PathBuf,
    pub created_at: DateTime<Utc>,
    /// When the session was last appended to. Derived from the log file's
    /// modification time, so it reflects every durable write for free.
    pub updated_at: DateTime<Utc>,
    /// The first user prompt, if the log contains one.
    pub first_user_message: Option<String>,
}

/// A loaded session: its metadata plus the full transcript.
#[derive(Debug, Clone)]
pub struct LoadedSession {
    pub summary: SessionSummary,
    pub messages: Vec<Message>,
    /// The log handle for appending new records.
    pub log: SessionLog,
}

/// The outcome of resolving a session-id prefix against a list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrefixMatch<'a> {
    /// No session matched the prefix.
    None,
    /// Exactly one session matched.
    Unique(&'a SessionSummary),
    /// Several sessions matched; the caller should ask for more input.
    Ambiguous(Vec<&'a SessionSummary>),
}

/// Resolve a session-id prefix against `sessions` (which must already be
/// scoped to the current repository). Empty prefixes match nothing.
pub fn resolve_prefix<'a>(sessions: &'a [SessionSummary], prefix: &str) -> PrefixMatch<'a> {
    if prefix.is_empty() {
        return PrefixMatch::None;
    }
    let matches: Vec<&SessionSummary> = sessions
        .iter()
        .filter(|summary| summary.id.matches_prefix(prefix))
        .collect();
    match matches.len() {
        0 => PrefixMatch::None,
        1 => PrefixMatch::Unique(matches[0]),
        _ => PrefixMatch::Ambiguous(matches),
    }
}

/// Where session logs live: `<data_dir>/vava/sessions/`.
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

    /// Create a new session log for the repository at `cwd` and write its
    /// header. The log lands in the repository's scoped directory.
    pub fn create(&self, cwd: &Path) -> Result<SessionLog, PersistError> {
        let id = Uuid::new_v4().to_string();
        let repo_dir = self.dir.join(repository_key(cwd));
        std::fs::create_dir_all(&repo_dir)?;
        let log = SessionLog {
            id: SessionId::new(id.clone()),
            path: repo_dir.join(format!("{id}.jsonl")),
        };
        let header = SessionHeader {
            id,
            cwd: cwd.display().to_string(),
            created_at: Some(Utc::now().to_rfc3339()),
        };
        let mut file = OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(&log.path)?;
        writeln!(file, "{}", header.to_record())?;
        file.flush()?;
        Ok(log)
    }

    /// The summary for a freshly created log (no messages yet).
    pub fn summary(&self, log: &SessionLog) -> Result<SessionSummary, PersistError> {
        let content = std::fs::read_to_string(&log.path)?;
        let header: SessionHeader = serde_json::from_str(content.lines().next().ok_or_else(
            || PersistError::Corrupt {
                id: log.id().to_string(),
                message: "session log is empty".into(),
            },
        )?)
        .map_err(|error| PersistError::Corrupt {
            id: log.id().to_string(),
            message: format!("bad header: {error}"),
        })?;
        summary_from_path(&log.path, &header, None)
    }

    /// Replay a session's transcript from its log.
    ///
    /// Tolerates a partially written final line (a crash mid-write); any
    /// other unreadable record is a corruption error.
    pub fn open_session(
        &self,
        id: impl AsRef<str>,
    ) -> Result<(SessionHeader, Vec<Message>), PersistError> {
        let path = self
            .find_path(id.as_ref())
            .ok_or_else(|| PersistError::NotFound {
                id: id.as_ref().to_string(),
            })?;
        self.read_log_file(&path)
    }

    /// Load a session by id: metadata plus the full transcript.
    pub fn load(&self, id: &SessionId) -> Result<LoadedSession, PersistError> {
        let path = self
            .find_path(id.as_str())
            .ok_or_else(|| PersistError::NotFound { id: id.to_string() })?;
        let (header, messages) = self.read_log_file(&path)?;
        let first_user = first_user_message(&messages);
        let summary = summary_from_path(&path, &header, first_user)?;
        let log = SessionLog {
            id: id.clone(),
            path,
        };
        Ok(LoadedSession {
            summary,
            messages,
            log,
        })
    }

    /// List every session of one repository, newest first.
    ///
    /// Sessions are found in the repository's scoped directory; legacy flat
    /// logs (written before repository scoping) are included when their
    /// header's `cwd` matches the repository root.
    pub fn list_for_repository(&self, root: &Path) -> Result<Vec<SessionSummary>, PersistError> {
        let key = repository_key(root);
        let repo_dir = self.dir.join(&key);
        let mut summaries = Vec::new();

        // 1. Sessions written with repository scoping.
        if repo_dir.is_dir() {
            for entry in std::fs::read_dir(&repo_dir)? {
                let entry = entry?;
                if entry.path().extension().is_some_and(|ext| ext == "jsonl") {
                    self.push_summary(&mut summaries, &entry.path());
                }
            }
        }

        // 2. Legacy flat logs, matched by the repository stored in their
        //    header. They are read in place and never rewritten.
        for entry in std::fs::read_dir(&self.dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() || !path.extension().is_some_and(|ext| ext == "jsonl") {
                continue;
            }
            if let Some((header, _)) = read_light(&path)
                && same_repository(&header.cwd, root)
            {
                self.push_summary(&mut summaries, &path);
            }
        }

        summaries.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        Ok(summaries)
    }

    /// The most recently updated session of one repository, if any.
    pub fn latest_for_repository(
        &self,
        root: &Path,
    ) -> Result<Option<SessionSummary>, PersistError> {
        Ok(self.list_for_repository(root)?.into_iter().next())
    }

    /// Push one log file's summary onto `summaries` (skipping unreadable
    /// files with a warning — they surface as errors when actually loaded).
    fn push_summary(&self, summaries: &mut Vec<SessionSummary>, path: &Path) {
        match read_light(path)
            .and_then(|(header, first_user)| summary_from_path(path, &header, first_user).ok())
        {
            Some(summary) => summaries.push(summary),
            None => tracing::warn!(path = %path.display(), "skipping unreadable session log"),
        }
    }

    /// Locate a session log by id: the legacy flat location first, then
    /// every repository-scoped directory.
    fn find_path(&self, id: &str) -> Option<PathBuf> {
        let flat = self.dir.join(format!("{id}.jsonl"));
        if flat.is_file() {
            return Some(flat);
        }
        for entry in std::fs::read_dir(&self.dir).ok()? {
            let path = entry.ok()?.path();
            if path.is_dir() {
                let candidate = path.join(format!("{id}.jsonl"));
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
        None
    }

    /// Read a log file's header and every message record.
    ///
    /// Tolerates a partially written final line; any other unreadable
    /// record is a corruption error. The file is never modified.
    fn read_log_file(&self, path: &Path) -> Result<(SessionHeader, Vec<Message>), PersistError> {
        let content = std::fs::read_to_string(path).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => PersistError::NotFound {
                id: path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            },
            _ => PersistError::Io(error),
        })?;

        let mut lines = content.lines();
        let header_line = lines.next().ok_or_else(|| PersistError::Corrupt {
            id: path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            message: "session log is empty".into(),
        })?;
        let header: SessionHeader =
            serde_json::from_str(header_line).map_err(|error| PersistError::Corrupt {
                id: path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                message: format!("bad header: {error}"),
            })?;

        let mut messages = Vec::new();
        for (index, line) in lines.enumerate() {
            match serde_json::from_str::<Message>(line) {
                Ok(message) => messages.push(message),
                Err(_) if is_last_line(&content, line) => break,
                Err(error) => {
                    return Err(PersistError::Corrupt {
                        id: header.id.clone(),
                        message: format!("record {}: {error}", index + 2),
                    });
                }
            }
        }
        Ok((header, messages))
    }

    /// List all sessions across every repository (header records), sorted
    /// by id. Used for debugging and legacy tests.
    pub fn list(&self) -> Result<Vec<SessionHeader>, PersistError> {
        let mut headers = Vec::new();
        let mut files = Vec::new();
        for entry in std::fs::read_dir(&self.dir)? {
            let path = entry?.path();
            if path.is_dir() {
                for nested in std::fs::read_dir(&path)? {
                    files.push(nested?.path());
                }
            } else {
                files.push(path);
            }
        }
        for path in files {
            if path.extension().is_some_and(|ext| ext == "jsonl")
                && let Some((header, _)) = read_light(&path)
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
    id: SessionId,
    path: PathBuf,
}

impl SessionLog {
    pub fn id(&self) -> &SessionId {
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

/// Read just enough of a log for listing: the header and, if present, the
/// first user message (which is always near the top of a real session).
/// Unreadable files yield `None` — listing is best-effort.
fn read_light(path: &Path) -> Option<(SessionHeader, Option<String>)> {
    let content = std::fs::read_to_string(path).ok()?;
    let mut lines = content.lines();
    let header: SessionHeader = serde_json::from_str(lines.next()?).ok()?;
    let mut first_user = None;
    for line in lines {
        if let Ok(Message::User(user)) = serde_json::from_str::<Message>(line) {
            first_user = Some(user.content);
            break;
        }
    }
    Some((header, first_user))
}

/// The first user prompt in a transcript, if any.
fn first_user_message(messages: &[Message]) -> Option<String> {
    messages.iter().find_map(|message| match message {
        Message::User(user) => Some(user.content.clone()),
        _ => None,
    })
}

/// Build a [`SessionSummary`] for one log file. `updated_at` is the
/// file's modification time (every append touches it); `created_at` comes
/// from the header, falling back to the file's modification time for logs
/// written before the field existed.
fn summary_from_path(
    path: &Path,
    header: &SessionHeader,
    first_user_message: Option<String>,
) -> Result<SessionSummary, PersistError> {
    let modified = std::fs::metadata(path)?
        .modified()
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let modified = DateTime::<Utc>::from(modified);
    let created_at = header
        .created_at
        .as_deref()
        .and_then(|stamp| DateTime::parse_from_rfc3339(stamp).ok())
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or(modified);
    Ok(SessionSummary {
        id: SessionId::new(header.id.clone()),
        repository_root: PathBuf::from(header.cwd.clone()),
        created_at,
        updated_at: modified,
        first_user_message,
    })
}

/// A stable, deterministic identity for a repository: the FNV-1a hash of
/// its canonicalized path. The hash is computed by hand so it never changes
/// across Rust releases (unlike `DefaultHasher`), keeping session
/// namespaces stable forever.
pub fn repository_key(root: &Path) -> String {
    let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in canonical.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Whether `header_cwd` refers to the same repository as `root`. The
/// header path was canonicalized when the session was created; the input is
/// canonicalized here so symlinked launches compare equal.
fn same_repository(header_cwd: &str, root: &Path) -> bool {
    let header = std::fs::canonicalize(header_cwd).unwrap_or_else(|_| PathBuf::from(header_cwd));
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    header == root
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
        assert_eq!(record["id"], log.id().as_str());
        assert_eq!(record["cwd"], "/projects/foo");
        assert!(record["created_at"].is_string());
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
    fn repository_scoping_keeps_sessions_separate() {
        let (store, _dir) = store();
        let repo_a = TestDir::new();
        let repo_b = TestDir::new();

        let a1 = store.create(repo_a.path()).unwrap();
        let a2 = store.create(repo_a.path()).unwrap();
        a1.append(&user("fix payments")).unwrap();
        let b1 = store.create(repo_b.path()).unwrap();
        b1.append(&user("refactor db")).unwrap();

        let for_a = store.list_for_repository(repo_a.path()).unwrap();
        assert_eq!(for_a.len(), 2);
        assert!(for_a.iter().all(|s| s.id == *a1.id() || s.id == *a2.id()));
        assert!(
            for_a
                .iter()
                .all(|s| s.first_user_message.as_deref() != Some("refactor db"))
        );

        let for_b = store.list_for_repository(repo_b.path()).unwrap();
        assert_eq!(for_b.len(), 1);
        assert_eq!(for_b[0].id, *b1.id());
    }

    #[test]
    fn nested_working_directory_resolves_to_the_same_repository() {
        let (store, _dir) = store();
        let repo = TestDir::new();
        std::fs::create_dir_all(repo.child(".git")).unwrap();
        std::fs::create_dir_all(repo.child("src/foo")).unwrap();

        // The caller resolves the workspace root (as CodingSession does);
        // launching from the nested directory yields the same root.
        let root = crate::find_repo_root(&repo.child("src/foo"));
        assert_eq!(root, repo.path());

        let log = store.create(&root).unwrap();
        log.append(&user("hello")).unwrap();

        let sessions = store.list_for_repository(&root).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, *log.id());
    }

    #[test]
    fn repository_key_is_canonical_and_stable() {
        let repo = TestDir::new();
        std::fs::create_dir_all(repo.child("sub")).unwrap();
        let key_root = repository_key(repo.path());
        // A nested path that is not the resolved root is a different key —
        // root resolution is the caller's job — but the key itself is
        // deterministic and canonical (symlink-free).
        assert_eq!(repository_key(repo.path()), key_root);
        assert_eq!(key_root.len(), 16);
        assert!(key_root.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn legacy_flat_sessions_are_still_listed_for_their_repository() {
        let (store, _dir) = store();
        let repo = TestDir::new();

        // Simulate a pre-scoping session: a flat file directly under the
        // store whose header names the repository.
        let id = Uuid::new_v4().to_string();
        let flat = store.dir().join(format!("{id}.jsonl"));
        let header = SessionHeader {
            id: id.clone(),
            cwd: repo.path().display().to_string(),
            created_at: None,
        };
        std::fs::write(&flat, format!("{}\n", header.to_record())).unwrap();
        append_log(&flat, &user("legacy prompt")).unwrap();

        let sessions = store.list_for_repository(repo.path()).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id.as_str(), id);
        assert_eq!(
            sessions[0].first_user_message.as_deref(),
            Some("legacy prompt")
        );
        // And it is loadable through the id.
        let loaded = store.load(&sessions[0].id).unwrap();
        assert_eq!(loaded.messages, vec![user("legacy prompt")]);
        // It was not moved or rewritten.
        assert!(flat.is_file());
    }

    #[test]
    fn latest_returns_the_most_recently_updated() {
        let (store, _dir) = store();
        let repo = TestDir::new();

        let older = store.create(repo.path()).unwrap();
        older.append(&user("first")).unwrap();
        let middle = store.create(repo.path()).unwrap();
        middle.append(&user("second")).unwrap();
        let newest = store.create(repo.path()).unwrap();
        newest.append(&user("third")).unwrap();

        // Force distinct modification times (filesystem mtime is the
        // updated_at source). Realistic, well-separated timestamps: some
        // filesystems clamp or round far-future values.
        let now = std::time::SystemTime::now();
        let stamps = [
            now - std::time::Duration::from_secs(120), // older
            now - std::time::Duration::from_secs(60),  // middle
            now,                                       // newest
        ];
        for (log, stamp) in [&older, &middle, &newest].into_iter().zip(stamps) {
            std::fs::File::open(log.path())
                .unwrap()
                .set_modified(stamp)
                .unwrap();
        }

        let latest = store.latest_for_repository(repo.path()).unwrap().unwrap();
        assert_eq!(latest.id, *newest.id());
        assert_eq!(latest.first_user_message.as_deref(), Some("third"));
    }

    #[test]
    fn prefix_resolution_detects_ambiguity_and_uniqueness() {
        let a = summary_with_id("01KABC1111");
        let b = summary_with_id("01KABC2222");
        let c = summary_with_id("01K9ZZ3333");
        let sessions = vec![a, b, c];

        // "01K" matches all three.
        match resolve_prefix(&sessions, "01K") {
            PrefixMatch::Ambiguous(matches) => assert_eq!(matches.len(), 3),
            other => panic!("expected ambiguous, got {other:?}"),
        }
        // "01KABC" matches two.
        match resolve_prefix(&sessions, "01KABC") {
            PrefixMatch::Ambiguous(matches) => assert_eq!(matches.len(), 2),
            other => panic!("expected ambiguous, got {other:?}"),
        }
        // "01K9ZZ" is unique.
        match resolve_prefix(&sessions, "01K9ZZ") {
            PrefixMatch::Unique(summary) => assert_eq!(summary.id.as_str(), "01K9ZZ3333"),
            other => panic!("expected unique, got {other:?}"),
        }
        // Unknown prefix matches nothing.
        assert!(matches!(
            resolve_prefix(&sessions, "zzz"),
            PrefixMatch::None
        ));
        assert!(matches!(resolve_prefix(&sessions, ""), PrefixMatch::None));
    }

    #[test]
    fn load_reconstructs_the_full_transcript() {
        let (store, _dir) = store();
        let log = store.create(&PathBuf::from("/p")).unwrap();
        log.append(&user("fix the tests")).unwrap();
        log.append(&assistant_with_reasoning_and_call()).unwrap();
        log.append(&Message::ToolResult(ToolResultMessage {
            tool_call_id: "call_1".into(),
            tool_name: "bash".into(),
            content: "passing".into(),
            is_error: false,
        }))
        .unwrap();
        log.append(&Message::Assistant(AssistantMessage::new("done")))
            .unwrap();

        let loaded = store.load(log.id()).unwrap();
        assert_eq!(loaded.summary.id, *log.id());
        assert_eq!(loaded.summary.repository_root, PathBuf::from("/p"));
        assert_eq!(
            loaded.summary.first_user_message.as_deref(),
            Some("fix the tests")
        );
        assert_eq!(
            loaded.messages,
            vec![
                user("fix the tests"),
                assistant_with_reasoning_and_call(),
                Message::ToolResult(ToolResultMessage {
                    tool_call_id: "call_1".into(),
                    tool_name: "bash".into(),
                    content: "passing".into(),
                    is_error: false,
                }),
                Message::Assistant(AssistantMessage::new("done")),
            ]
        );
    }

    #[test]
    fn truncated_final_line_does_not_break_load() {
        let (store, _dir) = store();
        let repo = TestDir::new();
        let log = store.create(repo.path()).unwrap();
        log.append(&user("first")).unwrap();
        let mut file = OpenOptions::new().append(true).open(log.path()).unwrap();
        writeln!(file, r#"{{"type":"assistant","content":"hel"#).unwrap();
        drop(file);

        let loaded = store.load(log.id()).unwrap();
        assert_eq!(loaded.messages, vec![user("first")]);
    }

    #[test]
    fn session_id_display_and_matching() {
        let id = SessionId::new("01KABC1234");
        assert_eq!(id.short(), "01KABC");
        assert_eq!(id.full(), "01KABC1234");
        assert!(id.matches_prefix("01K"));
        assert!(id.matches_prefix("01KABC1234"));
        assert!(!id.matches_prefix("01KABC1235"));
        assert_eq!(id.to_string(), "01KABC1234");
        // Short ids never panic on slicing.
        assert_eq!(SessionId::new("ab").short(), "ab");
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

    fn summary_with_id(id: &str) -> SessionSummary {
        SessionSummary {
            id: SessionId::new(id),
            repository_root: PathBuf::from("/p"),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            first_user_message: Some("hello".into()),
        }
    }
}
