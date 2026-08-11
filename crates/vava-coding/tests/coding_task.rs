//! A real coding task end to end: the agent loop with the *real* coding
//! tools (`read`, `write`, `edit`, `bash`) driving a temporary repository,
//! steered by a scripted fake model (no network).
//!
//! This proves the milestone-8 scenario: the loop genuinely reads, edits,
//! and runs commands against a repository.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use futures::stream::{self, BoxStream};
use tokio::sync::mpsc;

use vava_coding::tools::register_coding_tools;
use vava_core::{
    AgentError, AgentEvent, AgentHarness, Message, ModelClient, ModelEvent, ToolDefinition,
    ToolRegistry,
};

type BoxedError = Box<dyn std::error::Error + Send + Sync>;

/// A unique temporary repository that cleans itself up on drop.
struct TempRepo(PathBuf);

impl TempRepo {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("vava-repo-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
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

/// A scripted model: each `stream` call yields the next scripted turn.
struct ScriptedModel {
    turns: Vec<Vec<ModelEvent>>,
    next: AtomicUsize,
}

impl ScriptedModel {
    fn new(turns: Vec<Vec<ModelEvent>>) -> Self {
        Self {
            turns,
            next: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl ModelClient for ScriptedModel {
    async fn stream(
        &self,
        _messages: &[Message],
        _system: &str,
        _tools: &[ToolDefinition],
    ) -> Result<BoxStream<'static, Result<ModelEvent, BoxedError>>, BoxedError> {
        let index = self.next.fetch_add(1, Ordering::SeqCst);
        let events = self.turns.get(index).cloned().unwrap_or_default();
        Ok(Box::pin(stream::iter(events.into_iter().map(Ok))))
    }
}

/// The full event list for one scripted tool-call turn.
fn tool_turn(id: &str, name: &str, arguments: &str) -> Vec<ModelEvent> {
    vec![
        ModelEvent::ToolCallStarted {
            index: 0,
            id: id.into(),
            name: name.into(),
        },
        ModelEvent::ToolCallArgumentsDelta {
            index: 0,
            delta: arguments.into(),
        },
        ModelEvent::Finished,
    ]
}

fn harness(model: ScriptedModel, root: &Path) -> AgentHarness {
    let mut registry = ToolRegistry::new();
    register_coding_tools(&mut registry);
    AgentHarness::new(
        Arc::new(model),
        registry,
        "You are a coding agent.",
        root.to_path_buf(),
    )
}

async fn run_prompt(
    mut harness: AgentHarness,
    input: &str,
) -> (AgentHarness, Vec<AgentEvent>, Result<(), AgentError>) {
    let (tx, mut rx) = mpsc::channel(64);
    let result = harness.prompt(input.to_string(), tx).await;
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    (harness, events, result)
}

/// The full milestone scenario: read → edit → bash, then a final answer,
/// with the repository genuinely modified on disk.
#[tokio::test]
async fn read_edit_bash_loop_modifies_the_repository() {
    let repo = TempRepo::new();
    std::fs::write(
        repo.path().join("src/main.rs"),
        "fn main() { println!(\"old\"); }\n",
    )
    .unwrap();

    let model = ScriptedModel::new(vec![
        tool_turn("c1", "read", r#"{"path":"src/main.rs"}"#),
        tool_turn(
            "c2",
            "edit",
            r#"{"path":"src/main.rs","old_text":"println!(\"old\")","new_text":"println!(\"new\")"}"#,
        ),
        tool_turn("c3", "bash", r#"{"command":"grep -c new src/main.rs"}"#),
        vec![
            ModelEvent::ReasoningDelta("Fixed it.".into()),
            ModelEvent::TextDelta("The file now prints `new`.".into()),
            ModelEvent::Finished,
        ],
    ]);

    let (harness, events, result) =
        run_prompt(harness(model, repo.path()), "make main.rs print new").await;
    result.unwrap();

    // The repository really changed on disk.
    assert_eq!(
        std::fs::read_to_string(repo.path().join("src/main.rs")).unwrap(),
        "fn main() { println!(\"new\"); }\n"
    );

    // Transcript: user + 3 tool-call messages + 3 tool results + final.
    assert_eq!(harness.messages().len(), 1 + 3 + 3 + 1);
    let tool_results = harness
        .messages()
        .iter()
        .filter(|m| matches!(m, Message::ToolResult(_)))
        .count();
    assert_eq!(tool_results, 3);
    let Message::Assistant(final_message) = harness.messages().last().unwrap() else {
        panic!("expected final assistant message");
    };
    assert_eq!(final_message.content, "The file now prints `new`.");
    assert_eq!(
        final_message.reasoning_content.as_deref(),
        Some("Fixed it.")
    );

    // Events covered every tool call and its result.
    let started = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolCallStarted { .. }))
        .count();
    let finished = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolCallFinished { .. }))
        .count();
    assert_eq!(started, 3);
    assert_eq!(finished, 3);
    assert_eq!(events.last(), Some(&AgentEvent::TurnCompleted));
}

/// A failing command comes back to the model as an error result.
#[tokio::test]
async fn failed_bash_is_fed_back_to_the_model() {
    let repo = TempRepo::new();

    let model = ScriptedModel::new(vec![
        tool_turn("c1", "bash", r#"{"command":"exit 1"}"#),
        vec![
            ModelEvent::TextDelta("The command failed.".into()),
            ModelEvent::Finished,
        ],
    ]);

    let (harness, _, result) = run_prompt(harness(model, repo.path()), "run something").await;
    result.unwrap();

    let Message::ToolResult(tool_result) = &harness.messages()[2] else {
        panic!("expected a tool result message");
    };
    assert!(tool_result.is_error);
    assert!(tool_result.content.contains("exit code: 1"));

    let Message::Assistant(final_message) = &harness.messages()[3] else {
        panic!("expected the final assistant message");
    };
    assert_eq!(final_message.content, "The command failed.");
}

/// The tools reject paths outside the repository even when the model asks.
#[tokio::test]
async fn tools_never_escape_the_repository() {
    let repo = TempRepo::new();
    let outside = std::env::temp_dir().join(format!("vava-outside-{}", std::process::id()));
    std::fs::write(&outside, "secret").unwrap();

    let model = ScriptedModel::new(vec![
        tool_turn(
            "c1",
            "read",
            &format!(r#"{{"path":"{}"}}"#, outside.display()),
        ),
        vec![ModelEvent::TextDelta("done".into()), ModelEvent::Finished],
    ]);

    let (harness, _, result) = run_prompt(harness(model, repo.path()), "read the secret").await;
    result.unwrap();

    // The model got an error result, not the file contents.
    let Message::ToolResult(tool_result) = &harness.messages()[2] else {
        panic!("expected a tool result message");
    };
    assert!(tool_result.is_error);
    assert!(tool_result.content.contains("outside the workspace root"));
    assert!(!tool_result.content.contains("secret"));

    let _ = std::fs::remove_file(&outside);
}

/// Cancellation propagates from the harness through the registry into a
/// running `bash` process, which is killed.
#[tokio::test]
async fn cancelling_during_a_bash_call_kills_the_process() {
    let repo = TempRepo::new();
    let model = ScriptedModel::new(vec![tool_turn("c1", "bash", r#"{"command":"sleep 30"}"#)]);

    let mut registry = ToolRegistry::new();
    register_coding_tools(&mut registry);
    let mut harness = AgentHarness::new(
        Arc::new(model),
        registry,
        "You are a coding agent.",
        repo.path().to_path_buf(),
    );

    let token = harness.cancellation_token();
    let (tx, _rx) = mpsc::channel(16);
    let handle = tokio::spawn(async move { harness.prompt("do it".into(), tx).await });

    // Give the model call and the spawned `sleep 30` a moment to start.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    token.cancel();

    let result = handle.await.unwrap();
    assert!(matches!(result, Err(AgentError::Cancelled)));
}

/// `CodingSession` discovers the repository root from a nested directory,
/// loads `AGENTS.md`, and runs the tools against that root.
#[tokio::test]
async fn coding_session_discovers_root_and_agmd() {
    let repo = TempRepo::new();
    std::fs::create_dir_all(repo.path().join(".git")).unwrap();
    std::fs::write(repo.path().join("AGENTS.md"), "Use tabs.\n").unwrap();
    std::fs::write(repo.path().join("notes.txt"), "hello from the repo\n").unwrap();
    std::fs::create_dir_all(repo.path().join("nested/deep")).unwrap();

    let model = ScriptedModel::new(vec![
        tool_turn("c1", "read", r#"{"path":"notes.txt"}"#),
        vec![
            ModelEvent::TextDelta("Got it.".into()),
            ModelEvent::Finished,
        ],
    ]);
    let client: Arc<dyn ModelClient> = Arc::new(model);
    let mut session =
        vava_coding::CodingSession::open(client, &repo.path().join("nested/deep")).unwrap();

    // The session root is the repository root, not the nested directory.
    assert_eq!(session.root(), repo.path());
    assert_eq!(session.context().agents_md.as_deref(), Some("Use tabs.\n"));

    let (tx, mut rx) = mpsc::channel(64);
    let result = session.prompt("read notes".into(), tx).await;
    result.unwrap();
    while rx.recv().await.is_some() {}

    // The read tool resolved `notes.txt` against the discovered root.
    let Message::ToolResult(tool_result) = &session.messages()[2] else {
        panic!("expected a tool result message");
    };
    assert!(!tool_result.is_error, "{}", tool_result.content);
    assert!(tool_result.content.contains("hello from the repo"));
}

/// The session log is append-only JSONL: after a prompt, replaying the log
/// reconstructs the transcript exactly, reasoning content included.
#[tokio::test]
async fn session_is_persisted_and_replays() {
    let repo = TempRepo::new();
    std::fs::write(repo.path().join("notes.txt"), "hello\n").unwrap();

    let model = ScriptedModel::new(vec![
        tool_turn("c1", "bash", r#"{"command":"grep -c hello notes.txt"}"#),
        vec![
            ModelEvent::ReasoningDelta("Confirmed.".into()),
            ModelEvent::TextDelta("All good.".into()),
            ModelEvent::Finished,
        ],
    ]);

    let store_dir = TempRepo::new(); // reuse the unique-dir machinery
    let store = vava_coding::SessionStore::open_at(store_dir.path().to_path_buf()).unwrap();
    let client: Arc<dyn ModelClient> = Arc::new(model);
    let mut session =
        vava_coding::CodingSession::open_with_store(client, repo.path(), store).unwrap();

    let (tx, mut rx) = mpsc::channel(64);
    session.prompt("check the notes".into(), tx).await.unwrap();
    while rx.recv().await.is_some() {}

    // Replay the log and compare to the live transcript.
    let (header, replayed) = session
        .session_store()
        .open_session(session.session_id())
        .unwrap();
    assert_eq!(header.cwd, repo.path().to_str().unwrap());
    assert_eq!(replayed, session.messages());
    assert!(replayed.len() >= 3);

    // Reasoning survived persistence through a tool loop.
    let Message::Assistant(first) = &replayed[1] else {
        panic!("expected assistant message");
    };
    assert_eq!(first.tool_calls.len(), 1);
    let Message::Assistant(final_message) = &replayed[3] else {
        panic!("expected final assistant message");
    };
    assert_eq!(final_message.content, "All good.");
    assert_eq!(
        final_message.reasoning_content.as_deref(),
        Some("Confirmed.")
    );
}
