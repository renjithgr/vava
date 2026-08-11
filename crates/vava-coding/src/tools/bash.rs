//! The `bash` tool: run shell commands with the user's permissions.
//!
//! This tool is intentionally *not* sandboxed. Commands run from the
//! repository root with the full permissions of the user running vava. It
//! supports a timeout, a captured-output cap, and cancellation (the child
//! process is killed).

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use vava_core::{Tool, ToolContext, ToolError, ToolResult, parse_tool_args};

/// Default command timeout.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
/// Default captured output cap, per stream (stdout, stderr).
const DEFAULT_MAX_OUTPUT: usize = 32 * 1024;

/// `bash` — run a shell command.
pub struct BashTool {
    timeout: Duration,
    max_output: usize,
}

impl Default for BashTool {
    fn default() -> Self {
        Self::new(DEFAULT_TIMEOUT, DEFAULT_MAX_OUTPUT)
    }
}

impl BashTool {
    pub fn new(timeout: Duration, max_output: usize) -> Self {
        Self {
            timeout,
            max_output,
        }
    }
}

#[derive(Debug, Deserialize)]
struct BashParams {
    command: String,
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &'static str {
        "bash"
    }

    fn description(&self) -> &'static str {
        "Run a shell command from the repository root and return its output, \
         exit code, duration, and whether it timed out. Use this for builds, \
         tests, git, ls, grep, and anything else the other tools do not cover."
    }

    fn schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The command to run, as you would type it in a terminal"
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        if context.cancellation.is_cancelled() {
            return Err(ToolError::Cancelled);
        }
        let params: BashParams = parse_tool_args(self.name(), &input)?;
        if params.command.trim().is_empty() {
            return Err(ToolError::InvalidArguments {
                tool: self.name().to_string(),
                message: "command must not be empty".into(),
            });
        }

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string());
        let mut child = Command::new(&shell)
            .args(["-lc", &params.command])
            .current_dir(&context.root)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|error| ToolError::Execution {
                tool: self.name().to_string(),
                message: format!("could not spawn `{shell}`: {error}"),
            })?;

        // Read both pipes concurrently so a chatty child cannot deadlock.
        let stdout_task = tokio::spawn(read_capped(
            child.stdout.take().expect("stdout is piped"),
            self.max_output,
        ));
        let stderr_task = tokio::spawn(read_capped(
            child.stderr.take().expect("stderr is piped"),
            self.max_output,
        ));

        let started = std::time::Instant::now();
        let mut timed_out = false;
        let status = tokio::select! {
            biased;
            _ = context.cancellation.cancelled() => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(ToolError::Cancelled);
            }
            _ = tokio::time::sleep(self.timeout) => {
                timed_out = true;
                let _ = child.kill().await;
                let _ = child.wait().await;
                None
            }
            status = child.wait() => Some(status),
        };

        let (stdout, stdout_truncated) = stdout_task.await.unwrap_or_default();
        let (stderr, stderr_truncated) = stderr_task.await.unwrap_or_default();
        let duration = started.elapsed();
        let exit_code = status.and_then(Result::ok).and_then(|status| status.code());

        let mut content = String::new();
        if !stdout.is_empty() {
            content.push_str("[stdout]\n");
            content.push_str(&stdout);
            if !stdout.ends_with('\n') {
                content.push('\n');
            }
        }
        if !stderr.is_empty() {
            content.push_str("[stderr]\n");
            content.push_str(&stderr);
            if !stderr.ends_with('\n') {
                content.push('\n');
            }
        }
        match exit_code {
            Some(code) => content.push_str(&format!("exit code: {code}\n")),
            None => content.push_str("exit code: killed\n"),
        }
        content.push_str(&format!("duration: {:.2}s\n", duration.as_secs_f64()));
        if timed_out {
            content.push_str("timeout: true\n");
        }
        if stdout_truncated || stderr_truncated {
            content.push_str("(output truncated)\n");
        }

        Ok(ToolResult {
            content,
            is_error: timed_out || exit_code != Some(0),
        })
    }
}

/// Read a stream up to `max` bytes, reporting whether it was truncated.
async fn read_capped<R: AsyncReadExt + Unpin>(mut reader: R, max: usize) -> (String, bool) {
    let mut bytes = Vec::new();
    let mut truncated = false;
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                if bytes.len() + n > max {
                    let remaining = max - bytes.len();
                    bytes.extend_from_slice(&buf[..remaining]);
                    truncated = true;
                    break;
                }
                bytes.extend_from_slice(&buf[..n]);
            }
            Err(_) => break,
        }
    }
    (String::from_utf8_lossy(&bytes).into_owned(), truncated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    use crate::tools::test_util::TestDir;

    fn context(root: &std::path::Path, token: CancellationToken) -> ToolContext {
        ToolContext::new(root.to_path_buf(), token)
    }

    async fn run(input: Value, root: &std::path::Path) -> ToolResult {
        BashTool::default()
            .execute(input, &context(root, CancellationToken::new()))
            .await
            .expect("bash must not raise a hard error")
    }

    #[tokio::test]
    async fn captures_stdout_and_exit_code() {
        let dir = TestDir::new();
        let result = run(json!({"command": "echo hello"}), dir.path()).await;
        assert!(!result.is_error, "{}", result.content);
        assert!(result.content.contains("hello"));
        assert!(result.content.contains("exit code: 0"));
    }

    #[tokio::test]
    async fn captures_stderr() {
        let dir = TestDir::new();
        let result = run(json!({"command": "echo oops >&2"}), dir.path()).await;
        assert!(result.content.contains("[stderr]"));
        assert!(result.content.contains("oops"));
        assert!(result.content.contains("exit code: 0"));
    }

    #[tokio::test]
    async fn failing_commands_are_error_results() {
        let dir = TestDir::new();
        let result = run(json!({"command": "exit 3"}), dir.path()).await;
        assert!(result.is_error);
        assert!(result.content.contains("exit code: 3"));
    }

    #[tokio::test]
    async fn runs_from_the_repository_root() {
        let dir = TestDir::new();
        let result = run(json!({"command": "pwd"}), dir.path()).await;
        assert!(!result.is_error, "{}", result.content);
        assert!(result.content.contains(dir.path().to_str().unwrap()));
    }

    #[tokio::test]
    async fn times_out_and_kills_the_child() {
        let dir = TestDir::new();
        let tool = BashTool::new(Duration::from_millis(100), 8192);
        let result = tool
            .execute(
                json!({"command": "sleep 30"}),
                &context(dir.path(), CancellationToken::new()),
            )
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("timeout: true"));
        assert!(result.content.contains("exit code: killed"));
    }

    #[tokio::test]
    async fn caps_captured_output() {
        let dir = TestDir::new();
        let tool = BashTool::new(Duration::from_secs(10), 64);
        let result = tool
            .execute(
                json!({"command": "yes x | head -c 100000"}),
                &context(dir.path(), CancellationToken::new()),
            )
            .await
            .unwrap();
        assert!(result.content.contains("(output truncated)"));
    }

    #[tokio::test]
    async fn cancellation_kills_the_child() {
        let dir = TestDir::new();
        let token = CancellationToken::new();
        let context = context(dir.path(), token.clone());
        let handle = tokio::spawn(async move {
            BashTool::default()
                .execute(json!({"command": "sleep 30"}), &context)
                .await
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        token.cancel();
        let err = handle.await.unwrap().unwrap_err();
        assert!(matches!(err, ToolError::Cancelled));
    }

    #[tokio::test]
    async fn empty_command_is_typed_error() {
        let dir = TestDir::new();
        let err = BashTool::default()
            .execute(
                json!({"command": "   "}),
                &context(dir.path(), CancellationToken::new()),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments { tool, .. } if tool == "bash"));
    }
}
