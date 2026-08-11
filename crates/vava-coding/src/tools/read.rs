//! The `read` tool: read a text file within the repository, with line
//! numbers, optional offset and limit.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use vava_core::{Tool, ToolContext, ToolError, ToolResult, parse_tool_args};

/// `read` — read a file inside the repository and return it numbered.
pub struct ReadTool;

/// Lines returned when the model does not ask for a specific limit.
const DEFAULT_LIMIT: usize = 500;
/// Hard cap on lines returned in one call.
const MAX_LIMIT: usize = 2000;

#[derive(Debug, Deserialize)]
struct ReadParams {
    path: String,
    /// First line to read, 1-based.
    offset: Option<u32>,
    /// Maximum number of lines to read.
    limit: Option<u32>,
}

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &'static str {
        "read"
    }

    fn description(&self) -> &'static str {
        "Read a text file inside the repository and return it with line \
         numbers. Use `offset` (1-based first line) and `limit` to read a \
         section of a large file."
    }

    fn schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file, relative to the repository root"
                },
                "offset": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "First line to read, 1-based (default: 1)"
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Maximum number of lines to read (default: 500, capped at 2000)"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        if context.cancellation.is_cancelled() {
            return Err(ToolError::Cancelled);
        }
        let params: ReadParams = parse_tool_args(self.name(), &input)?;

        let path = match super::resolve_within_root(&context.root, &params.path) {
            Ok(path) => path,
            Err(error) => return Ok(ToolResult::error(error.to_string())),
        };

        let metadata = match tokio::fs::metadata(&path).await {
            Ok(metadata) => metadata,
            Err(error) => {
                return Ok(ToolResult::error(format!(
                    "could not stat `{}`: {error}",
                    path.display()
                )));
            }
        };
        if metadata.is_dir() {
            return Ok(ToolResult::error(format!(
                "`{}` is a directory, not a file; use the `bash` tool with \
                 `ls` to inspect directories",
                path.display()
            )));
        }

        let content = match tokio::fs::read_to_string(&path).await {
            Ok(content) => content,
            Err(error) => {
                return Ok(ToolResult::error(format!(
                    "could not read `{}`: {error}",
                    path.display()
                )));
            }
        };

        let lines: Vec<&str> = content.lines().collect();
        let total = lines.len();
        if total == 0 {
            return Ok(ToolResult::ok("(file is empty)"));
        }

        let limit = params
            .limit
            .map(|limit| limit as usize)
            .unwrap_or(DEFAULT_LIMIT)
            .min(MAX_LIMIT);
        let start = params.offset.unwrap_or(1).saturating_sub(1) as usize;
        if start >= total {
            return Ok(ToolResult::error(format!(
                "offset {} is past the end of the file ({total} lines)",
                start + 1
            )));
        }
        let end = start.saturating_add(limit).min(total);

        let width = total.to_string().len();
        let mut out = String::new();
        for (index, line) in lines[start..end].iter().enumerate() {
            out.push_str(&format!("{:>width$} | {line}\n", start + index + 1));
        }
        if end < total {
            out.push_str(&format!("... {} more lines\n", total - end));
        }
        Ok(ToolResult::ok(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::Path;
    use tokio_util::sync::CancellationToken;

    use crate::tools::test_util::TestDir;

    fn context(root: &Path) -> ToolContext {
        ToolContext::new(root.to_path_buf(), CancellationToken::new())
    }

    async fn run(input: Value, root: &Path) -> ToolResult {
        ReadTool
            .execute(input, &context(root))
            .await
            .expect("read must not raise a hard error")
    }

    #[tokio::test]
    async fn reads_a_file_with_line_numbers() {
        let dir = TestDir::new();
        dir.write("main.rs", "use anyhow::Result;\n\nfn main() {}\n");

        let result = run(json!({"path": "main.rs"}), dir.path()).await;
        assert!(!result.is_error, "{}", result.content);
        assert_eq!(
            result.content,
            "1 | use anyhow::Result;\n2 | \n3 | fn main() {}\n"
        );
    }

    #[tokio::test]
    async fn respects_offset_and_limit() {
        let dir = TestDir::new();
        let content = (1..=10)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        dir.write("f.txt", &content);

        let result = run(
            json!({"path": "f.txt", "offset": 3, "limit": 2}),
            dir.path(),
        )
        .await;
        assert_eq!(
            result.content,
            " 3 | line 3\n 4 | line 4\n... 6 more lines\n"
        );
    }

    #[tokio::test]
    async fn rejects_directories() {
        let dir = TestDir::new();

        let result = run(json!({"path": "."}), dir.path()).await;
        assert!(result.is_error);
        assert!(result.content.contains("directory"));
    }

    #[tokio::test]
    async fn reports_missing_files_as_results() {
        let dir = TestDir::new();

        let result = run(json!({"path": "nope.txt"}), dir.path()).await;
        assert!(result.is_error);
        assert!(result.content.contains("could not"));
    }

    #[tokio::test]
    async fn rejects_paths_outside_the_root() {
        let dir = TestDir::new();

        let result = run(json!({"path": "../outside.txt"}), dir.path()).await;
        assert!(result.is_error);
        assert!(result.content.contains("outside the workspace root"));
    }

    #[tokio::test]
    async fn reports_empty_files() {
        let dir = TestDir::new();
        dir.write("empty.txt", "");

        let result = run(json!({"path": "empty.txt"}), dir.path()).await;
        assert!(!result.is_error);
        assert_eq!(result.content, "(file is empty)");
    }

    #[tokio::test]
    async fn invalid_arguments_are_typed() {
        let dir = TestDir::new();

        let err = ReadTool
            .execute(json!({"offset": -1}), &context(dir.path()))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments { tool, .. } if tool == "read"));
    }
}
