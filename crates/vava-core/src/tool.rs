//! Tools: results, definitions, the [`Tool`] trait, and the registry.
//!
//! The [`Tool`] trait is the seam between the agent and anything it can do.
//! Tools are stateless and know nothing about DeepSeek or terminals: they
//! receive parsed arguments and a [`ToolContext`] (workspace boundary and
//! cancellation), and return a [`ToolResult`]. Ordinary failures (missing
//! file, failed test) are *results* with `is_error = true` so the model can
//! react to them; only infrastructural problems raise [`ToolError`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::error::ToolError;
use crate::message::ToolCall;

/// The outcome of executing one tool.
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

/// Everything a tool needs to know about its environment at execution time.
///
/// The harness builds one context per execution and passes it to every
/// tool, keeping tools stateless.
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// The workspace boundary: tools must never operate outside this
    /// directory.
    pub root: PathBuf,
    /// Checked by tools so cancellation propagates promptly.
    pub cancellation: CancellationToken,
}

impl ToolContext {
    pub fn new(root: PathBuf, cancellation: CancellationToken) -> Self {
        Self { root, cancellation }
    }
}

/// A tool the model can invoke.
///
/// Implementations must be `Send + Sync` and stateless.
#[async_trait]
pub trait Tool: Send + Sync {
    /// The name the model uses to invoke this tool.
    fn name(&self) -> &'static str;

    /// A description of what the tool does, for the model.
    fn description(&self) -> &'static str;

    /// A JSON Schema describing the arguments this tool accepts.
    fn schema(&self) -> serde_json::Value;

    /// Execute the tool.
    ///
    /// Ordinary failures are returned as [`ToolResult::error`] content so the
    /// model can react to them. Only infrastructural problems (cancellation,
    /// internal errors) raise [`ToolError`].
    async fn execute(
        &self,
        input: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError>;
}

/// Parse a tool call's raw arguments into a typed parameter struct.
///
/// Tools define plain `Deserialize` parameter structs; this is the single
/// place where a validation failure becomes a [`ToolError::InvalidArguments`].
pub fn parse_tool_args<T>(tool: &str, input: &serde_json::Value) -> Result<T, ToolError>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_value(input.clone()).map_err(|error| ToolError::InvalidArguments {
        tool: tool.to_string(),
        message: format!("expected arguments matching the tool schema: {error}"),
    })
}

/// The set of tools available to the agent in one session.
#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRegistry")
            .field("tools", &self.names())
            .finish()
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a tool. Registering a second tool with the same name
    /// replaces the first.
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// The names of all registered tools, sorted.
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.tools.keys().cloned().collect();
        names.sort();
        names
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Tool definitions in deterministic (name) order, for the API request.
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        let mut definitions: Vec<ToolDefinition> = self
            .tools
            .values()
            .map(|tool| ToolDefinition::new(tool.name(), tool.description(), tool.schema()))
            .collect();
        definitions.sort_by(|a, b| a.name.cmp(&b.name));
        definitions
    }

    /// Execute one tool call, returning structured errors for unknown tools,
    /// invalid arguments, and cancellation.
    pub async fn execute(
        &self,
        call: &ToolCall,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        if context.cancellation.is_cancelled() {
            return Err(ToolError::Cancelled);
        }
        let tool = self
            .tools
            .get(&call.name)
            .ok_or_else(|| ToolError::NotFound(call.name.clone()))?;
        tool.execute(call.arguments.clone(), context).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ------------------------------------------------------------ fake tools

    #[derive(Deserialize)]
    struct EchoParams {
        text: String,
    }

    /// A fake tool that echoes its argument.
    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &'static str {
            "echo"
        }

        fn description(&self) -> &'static str {
            "Echo back the input text."
        }

        fn schema(&self) -> serde_json::Value {
            json!({
                "type": "object",
                "properties": {"text": {"type": "string"}},
                "required": ["text"]
            })
        }

        async fn execute(
            &self,
            input: serde_json::Value,
            _context: &ToolContext,
        ) -> Result<ToolResult, ToolError> {
            let params: EchoParams = parse_tool_args(self.name(), &input)?;
            Ok(ToolResult::ok(format!("echo: {}", params.text)))
        }
    }

    /// A fake tool that fails hard (infrastructural error).
    struct HardFailTool;

    #[async_trait]
    impl Tool for HardFailTool {
        fn name(&self) -> &'static str {
            "hard_fail"
        }

        fn description(&self) -> &'static str {
            "Always fails with an execution error."
        }

        fn schema(&self) -> serde_json::Value {
            json!({"type": "object"})
        }

        async fn execute(
            &self,
            _input: serde_json::Value,
            _context: &ToolContext,
        ) -> Result<ToolResult, ToolError> {
            Err(ToolError::Execution {
                tool: self.name().to_string(),
                message: "something exploded".into(),
            })
        }
    }

    /// A fake tool that reports failure as a *result*, not an error.
    struct ReportFailTool;

    #[async_trait]
    impl Tool for ReportFailTool {
        fn name(&self) -> &'static str {
            "report_fail"
        }

        fn description(&self) -> &'static str {
            "Reports failure as a result."
        }

        fn schema(&self) -> serde_json::Value {
            json!({"type": "object"})
        }

        async fn execute(
            &self,
            _input: serde_json::Value,
            _context: &ToolContext,
        ) -> Result<ToolResult, ToolError> {
            Ok(ToolResult::error("exit code 1"))
        }
    }

    fn context() -> ToolContext {
        ToolContext::new(PathBuf::from("/tmp"), CancellationToken::new())
    }

    fn registry_with(tools: Vec<Arc<dyn Tool>>) -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        for tool in tools {
            registry.register(tool);
        }
        registry
    }

    // ---------------------------------------------------------------- tests

    #[tokio::test]
    async fn executes_a_registered_tool() {
        let registry = registry_with(vec![Arc::new(EchoTool)]);
        let call = ToolCall {
            id: "c1".into(),
            name: "echo".into(),
            arguments: json!({"text": "hello"}),
        };
        let result = registry.execute(&call, &context()).await.unwrap();
        assert_eq!(result.content, "echo: hello");
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn unknown_tool_is_a_structured_error() {
        let registry = registry_with(vec![Arc::new(EchoTool)]);
        let call = ToolCall::new("c1", "does_not_exist");
        let err = registry.execute(&call, &context()).await.unwrap_err();
        assert_eq!(err, ToolError::NotFound("does_not_exist".into()));
    }

    #[tokio::test]
    async fn invalid_arguments_are_rejected() {
        let registry = registry_with(vec![Arc::new(EchoTool)]);
        let call = ToolCall {
            id: "c1".into(),
            name: "echo".into(),
            arguments: json!({"wrong_field": 1}),
        };
        let err = registry.execute(&call, &context()).await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments { tool, .. } if tool == "echo"));
    }

    #[tokio::test]
    async fn hard_failures_raise_execution_error() {
        let registry = registry_with(vec![Arc::new(HardFailTool)]);
        let call = ToolCall::new("c1", "hard_fail");
        let err = registry.execute(&call, &context()).await.unwrap_err();
        assert!(matches!(err, ToolError::Execution { tool, .. } if tool == "hard_fail"));
    }

    #[tokio::test]
    async fn reported_failures_come_back_as_results() {
        // The model must be able to see a failed tool's output, so ordinary
        // failures are results with is_error set, not hard errors.
        let registry = registry_with(vec![Arc::new(ReportFailTool)]);
        let call = ToolCall::new("c1", "report_fail");
        let result = registry.execute(&call, &context()).await.unwrap();
        assert_eq!(result.content, "exit code 1");
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn cancelled_context_aborts_before_execution() {
        let token = CancellationToken::new();
        token.cancel();
        let context = ToolContext::new(PathBuf::from("/tmp"), token);
        let registry = registry_with(vec![Arc::new(EchoTool)]);
        let call = ToolCall::new("c1", "echo");
        let err = registry.execute(&call, &context).await.unwrap_err();
        assert_eq!(err, ToolError::Cancelled);
    }

    #[test]
    fn definitions_are_sorted_and_well_formed() {
        let registry = registry_with(vec![Arc::new(EchoTool), Arc::new(HardFailTool)]);
        let definitions = registry.definitions();
        assert_eq!(definitions.len(), 2);
        assert_eq!(definitions[0].name, "echo");
        assert_eq!(definitions[1].name, "hard_fail");
        assert!(definitions[0].description.contains("Echo"));
        assert_eq!(definitions[0].parameters["required"][0], "text");
    }

    #[test]
    fn register_replaces_same_name() {
        let mut registry = registry_with(vec![Arc::new(EchoTool)]);
        assert_eq!(registry.len(), 1);
        registry.register(Arc::new(HardFailTool));
        registry.register(Arc::new(EchoTool)); // replaces nothing (same name)
        assert_eq!(registry.len(), 2);
        assert_eq!(registry.names(), vec!["echo", "hard_fail"]);
    }

    #[test]
    fn empty_registry() {
        let registry = ToolRegistry::new();
        assert!(registry.is_empty());
        assert!(registry.definitions().is_empty());
    }

    #[test]
    fn tool_result_round_trips_through_json() {
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
