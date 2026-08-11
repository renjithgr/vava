//! Typed events emitted by the two moving parts of vava.
//!
//! - [`ModelEvent`] describes streaming output from the model, in
//!   provider-independent terms. `vava-deepseek` translates DeepSeek SSE
//!   chunks into these; a fake model in tests produces the same events.
//! - [`AgentEvent`] describes the agent conversation from the perspective of
//!   a frontend. The harness emits these; a CLI or TUI consumes them. The
//!   harness never prints — it only emits events.

use serde::{Deserialize, Serialize};

use crate::message::{AssistantMessage, ToolCall};
use crate::tool::ToolResult;

/// A single streaming event from the model.
///
/// Text, reasoning, and tool-call arguments all arrive as deltas that must be
/// accumulated by the consumer. Tool-call arguments may arrive fragmented and
/// must be accumulated per tool-call index before parsing the JSON.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelEvent {
    /// A chunk of final-answer text.
    TextDelta(String),
    /// A chunk of reasoning text (the model's chain of thought).
    ReasoningDelta(String),
    /// The start of a tool call, identified by its index in the message.
    ToolCallStarted {
        /// The tool call's index within the assistant message.
        index: usize,
        /// The call id (may be empty until the model provides it).
        id: String,
        /// The name of the tool.
        name: String,
    },
    /// A fragment of a tool call's JSON arguments.
    ToolCallArgumentsDelta {
        /// Which tool call these arguments belong to.
        index: usize,
        /// The fragment of raw JSON text.
        delta: String,
    },
    /// Token usage reported by the model for this response.
    Usage(Usage),
    /// The model finished streaming this response.
    Finished,
}

/// Token usage reported by the model for one response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

impl Usage {
    /// Usage with the total derived from prompt + completion.
    pub fn new(prompt_tokens: u64, completion_tokens: u64) -> Self {
        Self {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
        }
    }
}

/// The contract between the agent harness and any frontend.
///
/// Consumers (the print CLI, the REPL, and eventually the Ratatui TUI) all
/// receive exactly these events and nothing else. Events are emitted as they
/// happen, so a user sees tokens arrive while the model is still talking.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    /// The agent started a new model turn.
    TurnStarted,
    /// A chunk of final-answer text from the model.
    TextDelta { delta: String },
    /// A chunk of reasoning text from the model.
    ReasoningDelta { delta: String },
    /// The model requested a tool call. Arguments may still be streaming, so
    /// `call.arguments` may be empty or partial at this point.
    ToolCallStarted { call: ToolCall },
    /// A tool call finished executing.
    ToolCallFinished { call_id: String, result: ToolResult },
    /// A complete assistant message was assembled and stored in the transcript.
    AssistantMessageCompleted { message: AssistantMessage },
    /// The agent produced its final response and the turn is over.
    TurnCompleted,
    /// An unrecoverable error occurred; the turn is over.
    Error { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_new_computes_total() {
        let usage = Usage::new(10, 20);
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.completion_tokens, 20);
        assert_eq!(usage.total_tokens, 30);
    }

    #[test]
    fn usage_default_is_zero() {
        let usage = Usage::default();
        assert_eq!(usage.total_tokens, 0);
    }

    #[test]
    fn usage_round_trips_through_json() {
        let usage = Usage::new(123, 456);
        let s = serde_json::to_string(&usage).unwrap();
        let back: Usage = serde_json::from_str(&s).unwrap();
        assert_eq!(back, usage);
    }

    #[test]
    fn model_event_variants_are_cloneable() {
        let events = [
            ModelEvent::TextDelta("hi".into()),
            ModelEvent::ReasoningDelta("think".into()),
            ModelEvent::ToolCallStarted {
                index: 0,
                id: "c".into(),
                name: "read".into(),
            },
            ModelEvent::ToolCallArgumentsDelta {
                index: 0,
                delta: r#"{"path":"x"}"#.into(),
            },
            ModelEvent::Usage(Usage::new(1, 2)),
            ModelEvent::Finished,
        ];
        for event in &events {
            let _cloned = event.clone();
        }
    }

    #[test]
    fn agent_event_carries_call_and_result() {
        let started = AgentEvent::ToolCallStarted {
            call: ToolCall::new("c1", "read"),
        };
        match started {
            AgentEvent::ToolCallStarted { call } => assert_eq!(call.name, "read"),
            other => panic!("unexpected variant: {other:?}"),
        }

        let finished = AgentEvent::ToolCallFinished {
            call_id: "c1".into(),
            result: ToolResult::ok("hello"),
        };
        match finished {
            AgentEvent::ToolCallFinished { call_id, result } => {
                assert_eq!(call_id, "c1");
                assert_eq!(result.content, "hello");
                assert!(!result.is_error);
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }
}
