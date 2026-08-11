//! Session resume integration tests: `/new` and `/resume` semantics at the
//! `CodingSession` level, with a scripted fake model — no network.
//!
//! These prove the milestone's core behaviors:
//! - a new session gets a new id, keeps the repository, starts empty, and
//!   never touches the previous session's log
//! - after resuming, future messages are appended to the resumed session
//!   rather than the abandoned current one
//! - the restored transcript (reasoning and tool calls included) is exactly
//!   what the model receives on the next turn

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use futures::stream::{self, BoxStream};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use vava_coding::{CodingSession, SessionStore};
use vava_core::{Message, ModelClient, ModelEvent, ToolDefinition};

type BoxedError = Box<dyn std::error::Error + Send + Sync>;

/// A unique temporary repository that cleans itself up on drop.
struct TempRepo(PathBuf);

impl TempRepo {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("vava-session-repo-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dir = std::fs::canonicalize(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A scripted model that records every transcript it was asked to continue.
struct RecordingModel {
    turns: Vec<Vec<ModelEvent>>,
    next: AtomicUsize,
    /// Every `messages` slice passed to `stream`, cloned.
    seen: std::sync::Mutex<Vec<Vec<Message>>>,
}

impl RecordingModel {
    fn new(turns: Vec<Vec<ModelEvent>>) -> Self {
        Self {
            turns,
            next: AtomicUsize::new(0),
            seen: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn seen(&self) -> Vec<Vec<Message>> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait]
impl ModelClient for RecordingModel {
    async fn stream(
        &self,
        messages: &[Message],
        _system: &str,
        _tools: &[ToolDefinition],
    ) -> Result<BoxStream<'static, Result<ModelEvent, BoxedError>>, BoxedError> {
        self.seen.lock().unwrap().push(messages.to_vec());
        let index = self.next.fetch_add(1, Ordering::SeqCst);
        let events = self.turns.get(index).cloned().unwrap_or_default();
        Ok(Box::pin(stream::iter(events.into_iter().map(Ok))))
    }
}

fn final_answer(text: &str) -> Vec<ModelEvent> {
    vec![ModelEvent::TextDelta(text.into()), ModelEvent::Finished]
}

fn reasoning_tool_then_answer() -> Vec<Vec<ModelEvent>> {
    vec![
        vec![
            ModelEvent::ReasoningDelta("let me check".into()),
            ModelEvent::ToolCallStarted {
                index: 0,
                id: "call_1".into(),
                name: "bash".into(),
            },
            ModelEvent::ToolCallArgumentsDelta {
                index: 0,
                delta: r#"{"command":"echo hi"}"#.into(),
            },
            ModelEvent::Finished,
        ],
        final_answer("all good"),
    ]
}

fn store() -> (SessionStore, TempRepo) {
    let store_dir = TempRepo::new();
    let store = SessionStore::open_at(store_dir.path().to_path_buf()).unwrap();
    (store, store_dir)
}

async fn prompt(session: &mut CodingSession, input: &str) {
    let (tx, mut rx) = mpsc::channel(64);
    session
        .prompt(input.into(), tx, CancellationToken::new())
        .await
        .unwrap();
    while rx.recv().await.is_some() {}
}

/// A fresh session gets a new id, keeps the repository, starts with an
/// empty transcript, and leaves the previous session's log untouched.
#[tokio::test]
async fn new_session_replaces_without_mutating_the_previous() {
    let (store, _store_dir) = store();
    let repo = TempRepo::new();

    let client: Arc<dyn ModelClient> = Arc::new(RecordingModel::new(vec![final_answer("one")]));
    let mut session = CodingSession::open_with_store(client, repo.path(), store).unwrap();
    prompt(&mut session, "first prompt").await;
    let old_id = session.session_id().clone();
    let old_messages = session.messages().to_vec();

    let summary = session.begin_new_session().unwrap();

    // New id, same repository, empty transcript.
    assert_ne!(summary.id, old_id);
    assert_eq!(session.session_id(), &summary.id);
    assert_eq!(summary.repository_root, repo.path());
    assert!(session.messages().is_empty());

    // The old session's log still contains exactly its own messages.
    let (_, replayed) = session.session_store().open_session(&old_id).unwrap();
    assert_eq!(replayed, old_messages);
}

/// After `/resume`, new messages go to the resumed session's log, and the
/// abandoned current session's log never grows.
#[tokio::test]
async fn future_messages_append_to_the_resumed_session() {
    let (store, _store_dir) = store();
    let repo = TempRepo::new();

    let client: Arc<dyn ModelClient> = Arc::new(RecordingModel::new(vec![
        final_answer("from session A"),
        final_answer("from session B"),
        final_answer("continued in A"),
    ]));
    let mut session_a =
        CodingSession::open_with_store(client.clone(), repo.path(), store.clone()).unwrap();
    prompt(&mut session_a, "ask A").await;
    let a_id = session_a.session_id().clone();

    let mut session_b = CodingSession::open_with_store(client, repo.path(), store).unwrap();
    prompt(&mut session_b, "ask B").await;
    let b_id = session_b.session_id().clone();

    // Resume session A into the currently-running session B.
    let loaded = session_b.session_store().load(&a_id).unwrap();
    session_b.resume_into(loaded).unwrap();
    assert_eq!(session_b.session_id(), &a_id);

    // A new turn appends to A's log...
    prompt(&mut session_b, "continue A").await;

    let a_messages = session_b.session_store().load(&a_id).unwrap().messages;
    let b_messages = session_b.session_store().load(&b_id).unwrap().messages;

    assert_eq!(a_messages.len(), 4); // ask A + answer + continue A + answer
    assert_eq!(b_messages.len(), 2); // ask B + answer, never grown
    assert!(matches!(a_messages.last().unwrap(), Message::Assistant(_)));
    assert!(
        b_messages
            .iter()
            .all(|m| !matches!(m, Message::User(u) if u.content == "continue A"))
    );
}

/// The restored transcript — reasoning, tool calls, and tool results — is
/// exactly what the model receives on the first turn after resume.
#[tokio::test]
async fn restored_transcript_is_served_to_the_model_unchanged() {
    let (store, _store_dir) = store();
    let repo = TempRepo::new();

    let model = Arc::new(RecordingModel::new(reasoning_tool_then_answer()));
    let client: Arc<dyn ModelClient> = model.clone();
    let mut session = CodingSession::open_with_store(client, repo.path(), store).unwrap();
    prompt(&mut session, "do the work").await;
    let id = session.session_id().clone();
    let live_transcript = session.messages().to_vec();
    assert!(live_transcript.iter().any(|m| matches!(
        m,
        Message::Assistant(a) if a.tool_calls.len() == 1 && a.reasoning_content.is_some()
    )));
    assert!(
        live_transcript
            .iter()
            .any(|m| matches!(m, Message::ToolResult(_)))
    );

    // A brand-new CodingSession resumed from the same store.
    let loaded = session.session_store().load(&id).unwrap();
    let resumed_model = Arc::new(RecordingModel::new(vec![final_answer("done")]));
    let resumed_client: Arc<dyn ModelClient> = resumed_model.clone();
    let mut resumed = CodingSession::resume_with_store(
        resumed_client,
        repo.path(),
        session.session_store().clone(),
        loaded,
    )
    .unwrap();

    // The restored transcript equals the live one before shutdown.
    assert_eq!(resumed.messages(), &live_transcript[..]);

    prompt(&mut resumed, "continue").await;

    // The model saw [restored transcript, new user message] — nothing lost,
    // nothing reordered.
    let seen = resumed_model.seen();
    assert_eq!(seen.len(), 1);
    let mut expected = live_transcript.clone();
    expected.push(Message::User(vava_core::UserMessage {
        content: "continue".into(),
    }));
    assert_eq!(seen[0], expected);

    // Sanity: the session that only recorded turns never saw them mixed in.
    let original_seen = model.seen();
    assert_eq!(original_seen.len(), 2); // reasoning turn + final turn
}
