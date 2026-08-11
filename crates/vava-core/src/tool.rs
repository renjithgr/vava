//! Tool results and — in a later milestone — the [`Tool`] trait and
//! [`ToolRegistry`]. [`ToolDefinition`] is here already because the DeepSeek
//! request serializer needs a provider-independent description of a tool to
//! advertise to the model.

use serde::{Deserialize, Serialize};

/// A provider-independent description of a tool, used to advertise tools to
/// the model. The DeepSeek layer converts this into its wire format
/// (`{"type": "function", "function": {...}}`); no core type carries
/// protocol-specific serde annotations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// The name the model uses to invoke the tool.
    pub name: String,
    /// A description of what the tool does, for the model.
    pub description: String,
    /// A JSON Schema describing the arguments the tool accepts.
    pub parameters: serde_json::Value,
}

impl ToolDefinition {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
        }
    }
}

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

    #[test]
    fn tool_definition_round_trips() {
        let definition = ToolDefinition::new(
            "read",
            "Read a file within the repository.",
            serde_json::json!({"type": "object"}),
        );
        let s = serde_json::to_string(&definition).unwrap();
        let back: ToolDefinition = serde_json::from_str(&s).unwrap();
        assert_eq!(back, definition);
        assert_eq!(back.name, "read");
    }
}
