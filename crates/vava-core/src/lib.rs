//! vava-core — provider-independent core types for the vava coding agent.
//!
//! This crate holds the pieces of vava that do not care about *which* model
//! API is behind the agent:
//!
//! - [`message`] — the conversation transcript ([`Message`]) and its parts.
//! - [`event`]  — [`ModelEvent`] (streaming model output) and [`AgentEvent`]
//!   (the contract between the agent and any frontend).
//! - [`tool`]   — tool results, definitions, the [`Tool`] trait, and the
//!   [`ToolRegistry`].
//! - [`agent`]  — assembling complete assistant messages from stream events.
//! - [`harness`] — the [`AgentHarness`]: the agent loop over a model client.
//! - [`model_client`] — the small seam between the harness and a provider.
//! - [`error`]  — typed errors raised by this layer.
//!
//! Nothing here knows about DeepSeek, HTTP, SSE, files, or terminals.
//! [`vava-deepseek`] translates between these types and the DeepSeek wire
//! protocol; the harness orchestrates a conversation using only these types;
//! a CLI or TUI renders [`AgentEvent`]s.

pub mod agent;
pub mod error;
pub mod event;
pub mod harness;
pub mod message;
pub mod model_client;
pub mod tool;

pub use agent::AssistantBuilder;
pub use error::{AgentError, Cancelled, ToolError};
pub use event::{AgentEvent, ModelEvent, Usage};
pub use harness::AgentHarness;
pub use message::{AssistantMessage, Message, ToolCall, ToolResultMessage, UserMessage};
pub use model_client::{BoxedError, ModelClient};
pub use tool::{Tool, ToolContext, ToolDefinition, ToolRegistry, ToolResult, parse_tool_args};
