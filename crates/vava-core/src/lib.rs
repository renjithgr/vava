//! vava-core — provider-independent core types for the vava coding agent.
//!
//! This crate holds the pieces of vava that do not care about *which* model
//! API is behind the agent:
//!
//! - [`message`] — the conversation transcript ([`Message`]) and its parts.
//! - [`event`]  — [`ModelEvent`] (streaming model output) and [`AgentEvent`]
//!   (the contract between the agent and any frontend).
//! - [`tool`]   — tool results. The tool trait and registry arrive in a
//!   later milestone.
//! - [`error`]  — typed errors raised by this layer.
//!
//! Nothing here knows about DeepSeek, HTTP, SSE, files, or terminals.
//! [`vava-deepseek`] translates between these types and the DeepSeek wire
//! protocol; the harness orchestrates a conversation using only these types;
//! a CLI or TUI renders [`AgentEvent`]s.

pub mod error;
pub mod event;
pub mod message;
pub mod tool;

pub use error::{Cancelled, ToolError};
pub use event::{AgentEvent, ModelEvent, Usage};
pub use message::{AssistantMessage, Message, ToolCall, ToolResultMessage, UserMessage};
pub use tool::{ToolDefinition, ToolResult};
