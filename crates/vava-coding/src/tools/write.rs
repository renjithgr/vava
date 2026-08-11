//! The `write` tool: create or overwrite a text file inside the repository.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use vava_core::{Tool, ToolContext, ToolError, ToolResult, parse_tool_args};

/// `write` — create or overwrite a UTF-8 text file.
pub struct WriteTool;

#[derive(Debug, Deserialize)]
struct WriteParams {
    path: String,
    content: String,
}

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &'static str {
        "write"
    }

    fn description(&self) -> &'static str {
        "Create a new file or overwrite an existing one inside the repository \
         with the given UTF-8 text content. Parent directories are created as \
         needed. For small changes to an existing file, prefer `edit`."
    }

    fn schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file, relative to the repository root"
                },
                "content": {
                    "type": "string",
                    "description": "The full text content to write"
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        if context.cancellation.is_cancelled() {
            return Err(ToolError::Cancelled);
        }
        let params: WriteParams = parse_tool_args(self.name(), &input)?;

        let path = match super::resolve_within_root(&context.root, &params.path) {
            Ok(path) => path,
            Err(error) => return Ok(ToolResult::error(error.to_string())),
        };

        match super::atomic_write(&path, &params.content).await {
            Ok(()) => Ok(ToolResult::ok(format!(
                "wrote {} bytes to `{}`",
                params.content.len(),
                path.display()
            ))),
            Err(error) => Ok(ToolResult::error(format!(
                "could not write `{}`: {error}",
                path.display()
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    use crate::tools::test_util::TestDir;

    fn context(root: &std::path::Path) -> ToolContext {
        ToolContext::new(root.to_path_buf(), CancellationToken::new())
    }

    async fn run(input: Value, root: &std::path::Path) -> ToolResult {
        WriteTool
            .execute(input, &context(root))
            .await
            .expect("write must not raise a hard error")
    }

    #[tokio::test]
    async fn creates_a_new_file_with_parent_directories() {
        let dir = TestDir::new();
        let result = run(
            json!({"path": "src/new/file.rs", "content": "fn main() {}\n"}),
            dir.path(),
        )
        .await;
        assert!(!result.is_error, "{}", result.content);
        assert!(result.content.contains("wrote"));
        assert_eq!(
            std::fs::read_to_string(dir.child("src/new/file.rs")).unwrap(),
            "fn main() {}\n"
        );
    }

    #[tokio::test]
    async fn overwrites_an_existing_file() {
        let dir = TestDir::new();
        dir.write("f.txt", "old");
        let result = run(json!({"path": "f.txt", "content": "new"}), dir.path()).await;
        assert!(!result.is_error, "{}", result.content);
        assert_eq!(std::fs::read_to_string(dir.child("f.txt")).unwrap(), "new");
    }

    #[tokio::test]
    async fn writes_utf8_content() {
        let dir = TestDir::new();
        let content = "fn main() { println!(\"héllo wörld\"); }\n";
        let result = run(json!({"path": "u.txt", "content": content}), dir.path()).await;
        assert!(!result.is_error, "{}", result.content);
        assert_eq!(
            std::fs::read_to_string(dir.child("u.txt")).unwrap(),
            content
        );
    }

    #[tokio::test]
    async fn rejects_paths_outside_the_root() {
        let dir = TestDir::new();
        let result = run(json!({"path": "../evil.txt", "content": "x"}), dir.path()).await;
        assert!(result.is_error);
        assert!(result.content.contains("outside the workspace root"));
        assert!(!dir.child("evil.txt").exists());
    }

    #[tokio::test]
    async fn invalid_arguments_are_typed() {
        let dir = TestDir::new();
        let err = WriteTool
            .execute(json!({"path": "x.txt"}), &context(dir.path()))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments { tool, .. } if tool == "write"));
    }
}
