//! Tool results, and — in a later milestone — the [`Tool`] trait and
//! [`ToolRegistry`].
//!
//! Milestone 1 only defines [`ToolResult`], the value produced by executing
//! a tool. The trait and registry are added when the tools themselves are
//! implemented (milestone 4), so they are defined with the real tool set in
//! mind rather than speculatively.

use serde::{Deserialize, Serialize};

/// The outcome of executing one tool.
///
/// Tools do not raise hard errors for ordinary failures (a failed `cargo
/// test`, a missing file). They return an error result, which is handed back
/// to the model as transcript content so the model can react to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult {
    /// The tool's output, formatted as text.
    pub content: String,
    /// Whether the tool failed.
    pub is_error: bool,
}

impl ToolResult {
    /// A successful result.
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
        }
    }

    /// A failed result.
    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_result_flags_success() {
        let result = ToolResult::ok("all good");
        assert_eq!(result.content, "all good");
        assert!(!result.is_error);
    }

    #[test]
    fn error_result_flags_failure() {
        let result = ToolResult::error("exit code 1");
        assert_eq!(result.content, "exit code 1");
        assert!(result.is_error);
    }

    #[test]
    fn result_round_trips_through_json() {
        let result = ToolResult::error("boom");
        let s = serde_json::to_string(&result).unwrap();
        let back: ToolResult = serde_json::from_str(&s).unwrap();
        assert_eq!(back, result);
    }
}
