//! Integration tests for the agent loop, driven by a fake model — no API
//! calls, no network. These prove the core requirement:
//!
//! ```text
//! model → tool → tool result → model → … → final answer
//! ```
//!
//! works, with the transcript and the emitted events exactly right.

use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::{self, BoxStream};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use vava_core::{
    AgentError, AgentEvent, AgentHarness, AssistantMessage, Message, ModelClient, ModelEvent, Tool,
    ToolContext, ToolDefinition, ToolError, ToolRegistry, ToolResult, UserMessage,
};

type BoxedError = Box<dyn Error + Send + Sync>;

// ------------------------------------------------------------------ fakes

/// A scripted model: each call to `stream` yields the next scripted
/// response, in order.
struct FakeModel {
    responses: Vec<Vec<ModelEvent>>,
    next: AtomicUsize,
    calls: AtomicUsize,
}

impl FakeModel {
    fn new(responses: Vec<Vec<ModelEvent>>) -> Self {
        Self {
            responses,
            next: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
        }
    }

    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ModelClient for FakeModel {
    async fn stream(
        &self,
        _messages: &[Message],
        _system: &str,
        _tools: &[ToolDefinition],
    ) -> Result<BoxStream<'static, Result<ModelEvent, BoxedError>>, BoxedError> {
        let index = self.next.fetch_add(1, Ordering::SeqCst);
        self.calls.fetch_add(1, Ordering::SeqCst);
        let events = self.responses.get(index).cloned().unwrap_or_default();
        Ok(Box::pin(stream::iter(events.into_iter().map(Ok))))
    }
}

/// A fake `bash` tool: the first invocation fails, later ones succeed.
struct FakeBash {
    calls: AtomicUsize,
}

impl FakeBash {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl Tool for FakeBash {
    fn name(&self) -> &'static str {
        "bash"
    }

    fn description(&self) -> &'static str {
        "Run a shell command."
    }

    fn schema(&self) -> Value {
        json!({"type": "object"})
    }

    async fn execute(
        &self,
        _input: Value,
        _context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            Ok(ToolResult::error("exit code 101"))
        } else {
            Ok(ToolResult::ok("ok"))
        }
    }
}

/// A fake `read` tool with a canned result.
struct FakeRead;

#[async_trait]
impl Tool for FakeRead {
    fn name(&self) -> &'static str {
        "read"
    }

    fn description(&self) -> &'static str {
        "Read a file."
    }

    fn schema(&self) -> Value {
        json!({"type": "object"})
    }

    async fn execute(
        &self,
        _input: Value,
        _context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::ok("1 | fn main() {}"))
    }
}

/// A fake `edit` tool with a canned result.
struct FakeEdit;

#[async_trait]
impl Tool for FakeEdit {
    fn name(&self) -> &'static str {
        "edit"
    }

    fn description(&self) -> &'static str {
        "Edit a file."
    }

    fn schema(&self) -> Value {
        json!({"type": "object"})
    }

    async fn execute(
        &self,
        _input: Value,
        _context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::ok("edited"))
    }
}

// ------------------------------------------------------------------ driver

fn harness(model: FakeModel, tools: Vec<Arc<dyn Tool>>) -> AgentHarness {
    let mut registry = ToolRegistry::new();
    for tool in tools {
        registry.register(tool);
    }
    AgentHarness::new(
        Arc::new(model),
        registry,
        "You are a coding agent.",
        PathBuf::from("/tmp"),
    )
}

/// Run one prompt against the harness, collecting every agent event and the
/// final result.
async fn run_prompt(
    mut harness: AgentHarness,
    input: &str,
) -> (AgentHarness, Vec<AgentEvent>, Result<(), AgentError>) {
    let (tx, mut rx) = mpsc::channel(64);
    let input = input.to_string();
    let handle = tokio::spawn(async move {
        let result = harness.prompt(input, tx, CancellationToken::new()).await;
        (harness, result)
    });
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    let (harness, result) = handle.await.unwrap();
    (harness, events, result)
}

// ------------------------------------------------------------------ tests

#[tokio::test]
async fn single_turn_final_answer() {
    let model = FakeModel::new(vec![vec![
        ModelEvent::TextDelta("Hello ".into()),
        ModelEvent::TextDelta("world".into()),
        ModelEvent::Finished,
    ]]);
    let (harness, events, result) = run_prompt(harness(model, vec![]), "say hi").await;
    result.unwrap();

    // Transcript: user message then the final assistant message.
    assert_eq!(harness.messages().len(), 2);
    assert_eq!(
        harness.messages()[0],
        Message::User(UserMessage {
            content: "say hi".into()
        })
    );
    assert_eq!(
        harness.messages()[1],
        Message::Assistant(AssistantMessage::new("Hello world"))
    );

    // Events: tokens streamed, message completed, turn completed.
    assert_eq!(events[0], AgentEvent::TurnStarted);
    assert!(events.contains(&AgentEvent::TextDelta {
        delta: "Hello ".into()
    }));
    assert!(events.contains(&AgentEvent::TextDelta {
        delta: "world".into()
    }));
    let completed = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::AssistantMessageCompleted { .. }))
        .count();
    assert_eq!(completed, 1);
    assert_eq!(events.last(), Some(&AgentEvent::TurnCompleted));
}

#[tokio::test]
async fn tool_call_loop_reaches_final_answer() {
    let model = FakeModel::new(vec![
        // Turn 1: request a tool call.
        vec![
            ModelEvent::ToolCallStarted {
                index: 0,
                id: "call_1".into(),
                name: "bash".into(),
            },
            ModelEvent::ToolCallArgumentsDelta {
                index: 0,
                delta: r#"{"command":"cargo test"}"#.into(),
            },
            ModelEvent::Finished,
        ],
        // Turn 2: final answer.
        vec![
            ModelEvent::ReasoningDelta("Done.".into()),
            ModelEvent::TextDelta("Fixed it.".into()),
            ModelEvent::Finished,
        ],
    ]);
    let (harness, events, result) = run_prompt(
        harness(model, vec![Arc::new(FakeBash::new())]),
        "fix the tests",
    )
    .await;
    result.unwrap();

    // Transcript: user, assistant(tool call), tool result, assistant(final).
    assert_eq!(harness.messages().len(), 4);
    let Message::Assistant(first) = &harness.messages()[1] else {
        panic!("expected assistant message");
    };
    assert_eq!(first.tool_calls.len(), 1);
    assert_eq!(first.tool_calls[0].name, "bash");
    assert_eq!(
        first.tool_calls[0].arguments,
        json!({"command": "cargo test"})
    );
    let Message::ToolResult(tool_result) = &harness.messages()[2] else {
        panic!("expected tool result message");
    };
    assert_eq!(tool_result.tool_call_id, "call_1");
    assert_eq!(tool_result.tool_name, "bash");
    assert!(tool_result.is_error);
    assert_eq!(tool_result.content, "exit code 101");
    let Message::Assistant(final_message) = &harness.messages()[3] else {
        panic!("expected assistant message");
    };
    assert_eq!(final_message.content, "Fixed it.");
    assert!(final_message.tool_calls.is_empty());

    // Events cover the tool lifecycle.
    assert!(events.contains(&AgentEvent::ToolCallStarted {
        call: vava_core::ToolCall::new("call_1", "bash"),
    }));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolCallFinished { call_id, result }
            if call_id == "call_1" && result.is_error && result.content == "exit code 101"
    )));
    let completed = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::AssistantMessageCompleted { .. }))
        .count();
    assert_eq!(completed, 2);
    assert_eq!(events.last(), Some(&AgentEvent::TurnCompleted));
}

/// The multi-step scenario from the spec: bash fails → read → edit → bash
/// passes → final response.
#[tokio::test]
async fn multi_step_repair_loop() {
    let tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(FakeBash::new()),
        Arc::new(FakeRead),
        Arc::new(FakeEdit),
    ];
    let model = Arc::new(FakeModel::new(vec![
        // 1. run the tests → they fail
        tool_turn("call_1", "bash", r#"{"command":"cargo test"}"#),
        // 2. read the failing file
        tool_turn("call_2", "read", r#"{"path":"src/lib.rs"}"#),
        // 3. fix it
        tool_turn(
            "call_3",
            "edit",
            r#"{"path":"src/lib.rs","old_text":"x","new_text":"y"}"#,
        ),
        // 4. re-run the tests → they pass
        tool_turn("call_4", "bash", r#"{"command":"cargo test"}"#),
        // 5. final answer
        vec![
            ModelEvent::ReasoningDelta("Tests pass now.".into()),
            ModelEvent::TextDelta("The failure was fixed.".into()),
            ModelEvent::Finished,
        ],
    ]));
    let model_handle = model.clone();

    let mut registry = ToolRegistry::new();
    for tool in tools {
        registry.register(tool);
    }
    let harness = AgentHarness::new(
        model,
        registry,
        "You are a coding agent.",
        PathBuf::from("/tmp"),
    );
    let (harness, events, result) = run_prompt(harness, "run the tests, fix them, run again").await;
    result.unwrap();

    // Exactly five model calls: four tool turns and one final answer.
    assert_eq!(model_handle.call_count(), 5);
    // 5 model calls, 4 tool calls, 4 tool results.
    assert_eq!(harness.messages().len(), 1 + 5 + 4);
    // The second assistant message carried a real `read` call with parsed
    // arguments.
    let Message::Assistant(second) = &harness.messages()[3] else {
        panic!("expected assistant message");
    };
    assert_eq!(
        second.tool_calls[0].arguments,
        json!({"path": "src/lib.rs"})
    );
    let tool_results = harness
        .messages()
        .iter()
        .filter(|m| matches!(m, Message::ToolResult(_)))
        .count();
    assert_eq!(tool_results, 4);
    let tool_call_events = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolCallStarted { .. }))
        .count();
    assert_eq!(tool_call_events, 4);
    // First bash invocation failed, second succeeded.
    let failures = harness
        .messages()
        .iter()
        .filter_map(|m| match m {
            Message::ToolResult(r) if r.is_error => Some(r.tool_name.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(failures, vec!["bash"]);
    let Message::Assistant(final_message) = harness.messages().last().unwrap() else {
        panic!("expected final assistant message");
    };
    assert_eq!(final_message.content, "The failure was fixed.");
}

/// Reasoning content must survive tool-call loops in the transcript.
#[tokio::test]
async fn reasoning_survives_tool_loops() {
    let model = FakeModel::new(vec![
        vec![
            ModelEvent::ReasoningDelta("Let me inspect first.".into()),
            ModelEvent::ToolCallStarted {
                index: 0,
                id: "call_1".into(),
                name: "read".into(),
            },
            ModelEvent::ToolCallArgumentsDelta {
                index: 0,
                delta: r#"{"path":"src/main.rs"}"#.into(),
            },
            ModelEvent::Finished,
        ],
        vec![
            ModelEvent::ReasoningDelta("Now I know.".into()),
            ModelEvent::TextDelta("It's fine.".into()),
            ModelEvent::Finished,
        ],
    ]);
    let (harness, _, result) =
        run_prompt(harness(model, vec![Arc::new(FakeRead)]), "check main.rs").await;
    result.unwrap();

    let Message::Assistant(first) = &harness.messages()[1] else {
        panic!("expected assistant message");
    };
    // The tool-call message keeps its reasoning.
    assert_eq!(
        first.reasoning_content.as_deref(),
        Some("Let me inspect first.")
    );
    assert_eq!(first.tool_calls.len(), 1);
}

/// Cancellation stops a model that never responds.
#[tokio::test]
async fn cancellation_stops_a_hung_model() {
    struct HungModel;

    #[async_trait]
    impl ModelClient for HungModel {
        async fn stream(
            &self,
            _messages: &[Message],
            _system: &str,
            _tools: &[ToolDefinition],
        ) -> Result<BoxStream<'static, Result<ModelEvent, BoxedError>>, BoxedError> {
            Ok(Box::pin(stream::pending()))
        }
    }

    let mut harness = AgentHarness::new(
        Arc::new(HungModel),
        ToolRegistry::new(),
        "sys",
        PathBuf::from("/tmp"),
    );
    let token = CancellationToken::new();
    let token_for_task = token.clone();
    let (tx, _rx) = mpsc::channel(16);

    let handle = tokio::spawn(async move { harness.prompt("hi".into(), tx, token_for_task).await });
    tokio::time::sleep(Duration::from_millis(20)).await;
    token.cancel();
    let result = handle.await.unwrap();
    assert!(matches!(result, Err(AgentError::Cancelled)));
}

/// A client error mid-stream aborts the turn with a typed error.
#[tokio::test]
async fn client_error_aborts_the_turn() {
    struct BrokenModel;

    #[async_trait]
    impl ModelClient for BrokenModel {
        async fn stream(
            &self,
            _messages: &[Message],
            _system: &str,
            _tools: &[ToolDefinition],
        ) -> Result<BoxStream<'static, Result<ModelEvent, BoxedError>>, BoxedError> {
            Err(Box::new(std::io::Error::other("connection reset")))
        }
    }

    let harness = AgentHarness::new(
        Arc::new(BrokenModel),
        ToolRegistry::new(),
        "sys",
        PathBuf::from("/tmp"),
    );
    let (harness, events, result) = run_prompt(harness, "hi").await;
    assert!(matches!(result, Err(AgentError::Client(_))));
    assert!(events.iter().any(
        |e| matches!(e, AgentEvent::Error { message } if message.contains("connection reset"))
    ));
    // The failed turn leaves only the user message in the transcript.
    assert_eq!(harness.messages().len(), 1);
}

/// A restored harness starts with the persisted transcript, serves it to
/// the model on the next call, and never re-emits it through the sink.
#[tokio::test]
async fn restored_harness_continues_the_persisted_transcript() {
    struct CapturingModel {
        seen: std::sync::Mutex<Vec<Message>>,
    }

    #[async_trait]
    impl ModelClient for CapturingModel {
        async fn stream(
            &self,
            messages: &[Message],
            _system: &str,
            _tools: &[ToolDefinition],
        ) -> Result<BoxStream<'static, Result<ModelEvent, BoxedError>>, BoxedError> {
            *self.seen.lock().unwrap() = messages.to_vec();
            let events = vec![
                ModelEvent::TextDelta("continuing".into()),
                ModelEvent::Finished,
            ];
            Ok(Box::pin(stream::iter(events.into_iter().map(Ok))))
        }
    }

    // The persisted transcript: user, assistant with reasoning + a tool
    // call, tool result, assistant final response.
    let restored = vec![
        Message::User(UserMessage {
            content: "find the bug".into(),
        }),
        Message::Assistant(AssistantMessage {
            content: String::new(),
            reasoning_content: Some("let me inspect".into()),
            tool_calls: vec![vava_core::ToolCall::new("call_1", "read")],
        }),
        Message::ToolResult(vava_core::ToolResultMessage {
            tool_call_id: "call_1".into(),
            tool_name: "read".into(),
            content: "fn main() {}".into(),
            is_error: false,
        }),
        Message::Assistant(AssistantMessage::with_reasoning("found it", "done")),
    ];

    let model = Arc::new(CapturingModel {
        seen: std::sync::Mutex::new(Vec::new()),
    });
    let model_for_stream: Arc<dyn ModelClient> = model.clone();
    let mut harness = AgentHarness::restored(
        model_for_stream,
        ToolRegistry::new(),
        "You are a coding agent.",
        PathBuf::from("/tmp"),
        restored.clone(),
    );

    // The restored transcript is the live transcript.
    assert_eq!(harness.messages(), &restored[..]);

    // A sink installed after restore sees only new messages.
    let sink_seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink_for_harness = sink_seen.clone();
    harness.set_message_sink(move |message| sink_for_harness.lock().unwrap().push(message.clone()));

    let (harness, _, result) = run_prompt(harness, "continue from here").await;
    result.unwrap();

    // The model received the restored transcript plus the new prompt, in
    // order, nothing lost.
    let seen = model.seen.lock().unwrap().clone();
    let mut expected = restored.clone();
    expected.push(Message::User(UserMessage {
        content: "continue from here".into(),
    }));
    assert_eq!(seen, expected);

    // The transcript kept everything and appended the new turn.
    assert_eq!(harness.messages().len(), restored.len() + 2);

    // The sink saw exactly the two new records, not the restored ones.
    let sunk = sink_seen.lock().unwrap().clone();
    assert_eq!(sunk.len(), 2);
    assert!(matches!(&sunk[0], Message::User(u) if u.content == "continue from here"));
    assert!(matches!(&sunk[1], Message::Assistant(a) if a.content == "continuing"));
}

// ------------------------------------------------------------------ helpers

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
