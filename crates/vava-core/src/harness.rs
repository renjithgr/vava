//! The agent harness: runs a conversation against a model client.
//!
//! The harness is provider- and repository-agnostic. It talks to a
//! [`ModelClient`], executes whatever tools are registered, and reports
//! progress exclusively through [`AgentEvent`]s — it never prints and never
//! touches the filesystem itself.

use std::path::PathBuf;
use std::sync::Arc;

use futures::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::ToolError;
use crate::agent::AssistantBuilder;
use crate::error::AgentError;
use crate::event::{AgentEvent, ModelEvent};
use crate::message::{AssistantMessage, Message, ToolCall, ToolResultMessage, UserMessage};
use crate::model_client::ModelClient;
use crate::tool::{ToolContext, ToolRegistry, ToolResult};

/// A synchronous callback invoked for every completed transcript message
/// (used by `CodingSession` for JSONL persistence).
pub type MessageSink = Box<dyn Fn(&Message) + Send + Sync>;

/// Runs the agent conversation: transcript, model calls, tool execution.
///
/// The conversation flow:
///
/// ```text
/// model
///   → assistant streaming (events forwarded immediately)
///   → tool call detected
///   → complete assistant message stored
///   → tool executed, result stored
///   → model called again
///   → … until a final response with no tool calls
/// ```
pub struct AgentHarness {
    client: Arc<dyn ModelClient>,
    messages: Vec<Message>,
    tools: ToolRegistry,
    system_prompt: String,
    /// The workspace boundary handed to tools via [`ToolContext`].
    root: PathBuf,
    /// Optional callback receiving a copy of every completed transcript
    /// message (used by `CodingSession` for JSONL persistence).
    message_sink: Option<MessageSink>,
}

impl AgentHarness {
    pub fn new(
        client: Arc<dyn ModelClient>,
        tools: ToolRegistry,
        system_prompt: impl Into<String>,
        root: PathBuf,
    ) -> Self {
        Self {
            client,
            messages: Vec::new(),
            tools,
            system_prompt: system_prompt.into(),
            root,
            message_sink: None,
        }
    }

    /// Notify a callback for every completed transcript message.
    ///
    /// Used by `CodingSession` to append each message to the session log as
    /// it completes. The callback runs synchronously and must be cheap.
    pub fn set_message_sink(&mut self, sink: impl Fn(&Message) + Send + Sync + 'static) {
        self.message_sink = Some(Box::new(sink));
    }

    /// Record a completed message: notify the sink, then store it.
    fn record(&mut self, message: Message) {
        if let Some(sink) = &self.message_sink {
            sink(&message);
        }
        self.messages.push(message);
    }

    /// The conversation transcript so far.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Run one user prompt to completion: model turns, tool calls, and the
    /// final answer, streaming [`AgentEvent`]s to `event_tx`.
    ///
    /// `cancellation` scopes this single call: cancel it (from a Ctrl-C
    /// task, say) to stop the model stream and any running tools. The
    /// caller owns the token, so each call can get a fresh one — a
    /// cancelled turn never poisons the next.
    ///
    /// Returns `Ok` when the agent produced a final response. On error
    /// (cancellation, model client failure) an `AgentEvent::Error` is
    /// emitted first, and the transcript keeps only complete messages.
    pub async fn prompt(
        &mut self,
        input: String,
        event_tx: mpsc::Sender<AgentEvent>,
        cancellation: CancellationToken,
    ) -> Result<(), AgentError> {
        if cancellation.is_cancelled() {
            return Err(AgentError::Cancelled);
        }
        self.record(Message::User(UserMessage { content: input }));
        let _ = event_tx.send(AgentEvent::TurnStarted).await;

        loop {
            let assistant = match self.run_turn(&event_tx, &cancellation).await {
                Ok(assistant) => assistant,
                Err(error) => {
                    let _ = event_tx
                        .send(AgentEvent::Error {
                            message: error.to_string(),
                        })
                        .await;
                    return Err(error);
                }
            };

            self.record(Message::Assistant(assistant.clone()));
            let _ = event_tx
                .send(AgentEvent::AssistantMessageCompleted {
                    message: assistant.clone(),
                })
                .await;

            if assistant.tool_calls.is_empty() {
                break; // final response
            }

            if let Err(error) = self
                .execute_tool_calls(&assistant, &event_tx, &cancellation)
                .await
            {
                let _ = event_tx
                    .send(AgentEvent::Error {
                        message: error.to_string(),
                    })
                    .await;
                return Err(error);
            }
        }

        let _ = event_tx.send(AgentEvent::TurnCompleted).await;
        Ok(())
    }

    /// One model call: forward events to the frontend while assembling the
    /// complete assistant message.
    async fn run_turn(
        &self,
        event_tx: &mpsc::Sender<AgentEvent>,
        cancellation: &CancellationToken,
    ) -> Result<AssistantMessage, AgentError> {
        let tools = self.tools.definitions();
        let mut stream = self
            .client
            .stream(&self.messages, &self.system_prompt, &tools)
            .await
            .map_err(AgentError::Client)?;

        let mut builder = AssistantBuilder::new();
        loop {
            let event = tokio::select! {
                biased;
                _ = cancellation.cancelled() => {
                    return Err(AgentError::Cancelled);
                }
                next = stream.next() => {
                    match next {
                        Some(Ok(event)) => event,
                        Some(Err(error)) => return Err(AgentError::Client(error)),
                        None => break,
                    }
                }
            };
            self.forward(&event, event_tx).await;
            builder.push(event);
        }
        Ok(builder.build())
    }

    /// Translate a model event into the corresponding agent event, if any.
    ///
    /// Tool-call argument fragments and usage are internal details; the
    /// frontend learns about a tool call when it starts and when it
    /// finishes.
    async fn forward(&self, event: &ModelEvent, event_tx: &mpsc::Sender<AgentEvent>) {
        let agent_event = match event {
            ModelEvent::TextDelta(delta) => Some(AgentEvent::TextDelta {
                delta: delta.clone(),
            }),
            ModelEvent::ReasoningDelta(delta) => Some(AgentEvent::ReasoningDelta {
                delta: delta.clone(),
            }),
            ModelEvent::ToolCallStarted { id, name, .. } => Some(AgentEvent::ToolCallStarted {
                call: ToolCall::new(id.clone(), name.clone()),
            }),
            ModelEvent::ToolCallArgumentsDelta { .. }
            | ModelEvent::Usage(_)
            | ModelEvent::Finished => None,
        };
        if let Some(agent_event) = agent_event {
            let _ = event_tx.send(agent_event).await;
        }
    }

    /// Execute every tool call of an assistant message, appending results
    /// to the transcript. Hard tool errors are converted into error results
    /// so the model can see and react to them — except cancellation, which
    /// aborts the turn.
    async fn execute_tool_calls(
        &mut self,
        assistant: &AssistantMessage,
        event_tx: &mpsc::Sender<AgentEvent>,
        cancellation: &CancellationToken,
    ) -> Result<(), AgentError> {
        let context = ToolContext::new(self.root.clone(), cancellation.clone());
        for call in &assistant.tool_calls {
            let result = match self.tools.execute(call, &context).await {
                Ok(result) => result,
                Err(ToolError::Cancelled) => return Err(AgentError::Cancelled),
                Err(error) => ToolResult::error(error.to_string()),
            };
            let _ = event_tx
                .send(AgentEvent::ToolCallFinished {
                    call_id: call.id.clone(),
                    result: result.clone(),
                })
                .await;
            self.record(Message::ToolResult(ToolResultMessage::from_call(
                call, result,
            )));
        }
        Ok(())
    }
}
