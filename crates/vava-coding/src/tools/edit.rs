//! The `edit` tool: exact string replacement in a file inside the repository.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use vava_core::{Tool, ToolContext, ToolError, ToolResult, parse_tool_args};

/// `edit` — replace one exact occurrence of `old_text` with `new_text`.
///
/// The replacement must match exactly once: zero matches and ambiguous
/// (multiple) matches are both errors. No fuzzy matching.
pub struct EditTool;

#[derive(Debug, Deserialize)]
struct EditParams {
    path: String,
    old_text: String,
    new_text: String,
}

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &'static str {
        "edit"
    }

    fn description(&self) -> &'static str {
        "Replace one exact occurrence of `old_text` with `new_text` in a file \
         inside the repository. `old_text` must appear exactly once; include \
         enough surrounding context to make it unique."
    }

    fn schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file, relative to the repository root"
                },
                "old_text": {
                    "type": "string",
                    "description": "The exact text to replace (must appear exactly once)"
                },
                "new_text": {
                    "type": "string",
                    "description": "The replacement text"
                }
            },
            "required": ["path", "old_text", "new_text"]
        })
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        if context.cancellation.is_cancelled() {
            return Err(ToolError::Cancelled);
        }
        let params: EditParams = parse_tool_args(self.name(), &input)?;

        if params.old_text.is_empty() {
            return Ok(ToolResult::error(
                "`old_text` must not be empty; include the exact text to replace",
            ));
        }

        let path = match super::resolve_within_root(&context.root, &params.path) {
            Ok(path) => path,
            Err(error) => return Ok(ToolResult::error(error.to_string())),
        };

        let content = match tokio::fs::read_to_string(&path).await {
            Ok(content) => content,
            Err(error) => {
                return Ok(ToolResult::error(format!(
                    "could not read `{}`: {error}",
                    path.display()
                )));
            }
        };

        let occurrences = content.matches(&params.old_text).count();
        if occurrences == 0 {
            return Ok(ToolResult::error(format!(
                "`old_text` was not found in `{}`; read the file and use text that exists there",
                path.display()
            )));
        }
        if occurrences > 1 {
            return Ok(ToolResult::error(format!(
                "`old_text` occurs {occurrences} times in `{}`; include more surrounding \
                 context (read the file first) so it matches exactly once",
                path.display()
            )));
        }

        let updated = content.replacen(&params.old_text, &params.new_text, 1);
        match super::atomic_write(&path, &updated).await {
            Ok(()) => Ok(ToolResult::ok(format!("edited `{}`", path.display()))),
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
        EditTool
            .execute(input, &context(root))
            .await
            .expect("edit must not raise a hard error")
    }

    #[tokio::test]
    async fn replaces_a_unique_occurrence() {
        let dir = TestDir::new();
        dir.write("main.rs", "fn main() {\n    println!(\"old\");\n}\n");
        let result = run(
            json!({
                "path": "main.rs",
                "old_text": "println!(\"old\")",
                "new_text": "println!(\"new\")"
            }),
            dir.path(),
        )
        .await;
        assert!(!result.is_error, "{}", result.content);
        assert_eq!(
            std::fs::read_to_string(dir.child("main.rs")).unwrap(),
            "fn main() {\n    println!(\"new\");\n}\n"
        );
    }

    #[tokio::test]
    async fn fails_when_old_text_is_missing() {
        let dir = TestDir::new();
        dir.write("f.txt", "hello world");
        let result = run(
            json!({"path": "f.txt", "old_text": "nope", "new_text": "x"}),
            dir.path(),
        )
        .await;
        assert!(result.is_error);
        assert!(result.content.contains("was not found"));
    }

    #[tokio::test]
    async fn fails_when_old_text_is_ambiguous() {
        let dir = TestDir::new();
        dir.write("f.txt", "hello hello world");
        let result = run(
            json!({"path": "f.txt", "old_text": "hello", "new_text": "bye"}),
            dir.path(),
        )
        .await;
        assert!(result.is_error);
        assert!(result.content.contains("occurs 2 times"));
        assert!(result.content.contains("surrounding context"));
        // The file is untouched.
        assert_eq!(
            std::fs::read_to_string(dir.child("f.txt")).unwrap(),
            "hello hello world"
        );
    }

    #[tokio::test]
    async fn rejects_empty_old_text() {
        let dir = TestDir::new();
        dir.write("f.txt", "abc");
        let result = run(
            json!({"path": "f.txt", "old_text": "", "new_text": "x"}),
            dir.path(),
        )
        .await;
        assert!(result.is_error);
        assert!(result.content.contains("must not be empty"));
    }

    #[tokio::test]
    async fn edits_utf8_content() {
        let dir = TestDir::new();
        dir.write("u.txt", "héllo wörld");
        let result = run(
            json!({"path": "u.txt", "old_text": "wörld", "new_text": "world"}),
            dir.path(),
        )
        .await;
        assert!(!result.is_error, "{}", result.content);
        assert_eq!(
            std::fs::read_to_string(dir.child("u.txt")).unwrap(),
            "héllo world"
        );
    }

    #[tokio::test]
    async fn rejects_paths_outside_the_root() {
        let dir = TestDir::new();
        let result = run(
            json!({"path": "../evil.txt", "old_text": "a", "new_text": "b"}),
            dir.path(),
        )
        .await;
        assert!(result.is_error);
        assert!(result.content.contains("outside the workspace root"));
    }
}
