//! Typed errors raised by the core layer.
//!
//! The full error taxonomy spans the whole project:
//!
//! | error                    | lives in                          |
//! |--------------------------|-----------------------------------|
//! | tool not found           | this crate ([`ToolError`])        |
//! | tool argument validation | this crate ([`ToolError`])        |
//! | cancellation             | this crate ([`Cancelled`])        |
//! | HTTP / API               | `vava-deepseek`                   |
//! | SSE parsing              | `vava-deepseek`                   |
//! | DeepSeek protocol        | `vava-deepseek`                   |
//! | configuration            | `vava-coding` / `vava-cli`        |
//! | filesystem / workspace   | `vava-coding` (the coding tools)  |
//! | process / timeout        | `vava-coding` (the `bash` tool)   |
//! | session persistence      | `vava-coding`                     |
//!
//! Each layer only defines the errors it can actually raise, keeping error
//! types small and specific instead of one giant catch-all.

use thiserror::Error;

use crate::model_client::BoxedError;

/// A marker for an operation cancelled by the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("operation cancelled")]
pub struct Cancelled;

/// Errors raised by the agent harness while running a conversation.
#[derive(Debug, Error)]
pub enum AgentError {
    /// The operation was cancelled by the user.
    #[error("operation cancelled")]
    Cancelled,

    /// The model client failed (HTTP, API, protocol, ...).
    #[error("model client error: {0}")]
    Client(BoxedError),
}

/// Errors raised by the tool layer: resolving a tool, validating its
/// arguments, or executing it.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ToolError {
    /// The model asked for a tool that is not registered.
    #[error("unknown tool: `{0}`")]
    NotFound(String),

    /// The tool call's arguments did not validate.
    #[error("invalid arguments for tool `{tool}`: {message}")]
    InvalidArguments {
        /// The name of the tool that rejected the arguments.
        tool: String,
        /// Why the arguments were rejected.
        message: String,
    },

    /// The tool could not be executed.
    #[error("tool `{tool}` failed: {message}")]
    Execution {
        /// The name of the tool that failed.
        tool: String,
        /// What went wrong.
        message: String,
    },

    /// The operation was cancelled before the tool ran.
    #[error("tool execution cancelled")]
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_displays_cleanly() {
        assert_eq!(Cancelled.to_string(), "operation cancelled");
    }

    #[test]
    fn agent_error_displays() {
        assert_eq!(AgentError::Cancelled.to_string(), "operation cancelled");
        assert_eq!(
            AgentError::Client(Box::new(SimpleError)).to_string(),
            "model client error: kaput"
        );
    }

    #[derive(Debug)]
    struct SimpleError;

    impl std::fmt::Display for SimpleError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("kaput")
        }
    }

    impl std::error::Error for SimpleError {}

    #[test]
    fn tool_error_displays_are_helpful() {
        let not_found = ToolError::NotFound("grep".into());
        assert_eq!(not_found.to_string(), "unknown tool: `grep`");

        let invalid = ToolError::InvalidArguments {
            tool: "read".into(),
            message: "`path` is required".into(),
        };
        assert_eq!(
            invalid.to_string(),
            "invalid arguments for tool `read`: `path` is required"
        );

        let failed = ToolError::Execution {
            tool: "bash".into(),
            message: "timed out".into(),
        };
        assert_eq!(failed.to_string(), "tool `bash` failed: timed out");
    }

    #[test]
    fn tool_error_is_comparable() {
        assert_eq!(
            ToolError::NotFound("read".into()),
            ToolError::NotFound("read".into())
        );
        assert_ne!(
            ToolError::NotFound("read".into()),
            ToolError::NotFound("write".into())
        );
    }
}
