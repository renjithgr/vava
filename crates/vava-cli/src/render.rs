//! Rendering of [`AgentEvent`]s for the print CLI.
//!
//! This is the simplest frontend: text streaming to stdout, with the blocks
//! separated by blank lines:
//!
//! ```text
//! > Fix the failing tests
//!
//! Thinking...
//!
//! ● Bash
//!   cargo test
//!
//!   exit code: 101
//!
//! The failing test was caused by...
//! ```
//!
//! Reasoning is shown as a "Thinking..." marker by default; `--debug` streams
//! the reasoning text itself. The REPL and the Ratatui TUI will render the
//! same events differently — the events are the contract, not the rendering.

use std::io::Write;

use tokio::sync::mpsc;

use vava_core::{AgentEvent, ToolResult};

/// How many lines of a tool result are shown before truncating.
const RESULT_PREVIEW_LINES: usize = 8;
/// How many lines are shown when a result is truncated.
const RESULT_TRUNCATED_LINES: usize = 4;

/// Renders agent events to a text stream.
pub struct Renderer {
    /// Whether reasoning deltas are shown in full (the `--debug` flag).
    show_reasoning: bool,
    /// Whether the "Thinking..." marker was printed for the current turn.
    thinking_announced: bool,
}

impl Renderer {
    pub fn new(show_reasoning: bool) -> Self {
        Self {
            show_reasoning,
            thinking_announced: false,
        }
    }

    /// Render the user prompt as a header line.
    pub fn render_prompt(&self, prompt: &str, out: &mut impl Write) -> std::io::Result<()> {
        writeln!(out, "> {prompt}\n")
    }

    /// Render one event.
    pub fn render(&mut self, event: &AgentEvent, out: &mut impl Write) -> std::io::Result<()> {
        match event {
            AgentEvent::TurnStarted => {
                self.thinking_announced = false;
                Ok(())
            }
            AgentEvent::TextDelta { delta } => write!(out, "{delta}"),
            AgentEvent::ReasoningDelta { delta } => {
                if self.show_reasoning {
                    write!(out, "{delta}")
                } else if !self.thinking_announced {
                    self.thinking_announced = true;
                    writeln!(out, "Thinking...\n")
                } else {
                    Ok(())
                }
            }
            AgentEvent::ToolCallStarted { call } => {
                self.thinking_announced = false;
                writeln!(out, "● {}", render_call(&call.name, &call.arguments))
            }
            AgentEvent::ToolCallFinished { result, .. } => {
                writeln!(out)?;
                render_result(result, out)
            }
            AgentEvent::AssistantMessageCompleted { .. } => Ok(()),
            AgentEvent::TurnCompleted => writeln!(out),
            AgentEvent::Error { message } => writeln!(out, "\nerror: {message}\n"),
        }
    }
}

/// Render the event stream to stdout until the channel closes.
pub async fn render_events(
    mut rx: mpsc::Receiver<AgentEvent>,
    show_reasoning: bool,
    prompt: String,
) {
    let mut renderer = Renderer::new(show_reasoning);
    {
        let stdout = std::io::stdout();
        let mut out = stdout.lock();
        let _ = renderer.render_prompt(&prompt, &mut out);
        let _ = out.flush();
    }
    while let Some(event) = rx.recv().await {
        // The stdout lock is taken per event so it is never held across an
        // await point (which would make this future !Send).
        let stdout = std::io::stdout();
        let mut out = stdout.lock();
        let _ = renderer.render(&event, &mut out);
        let _ = out.flush();
    }
}

/// One line describing a tool call. Bash-style commands (an arguments
/// object with a single `command` key) get their own indented line.
fn render_call(name: &str, arguments: &serde_json::Value) -> String {
    if let serde_json::Value::Object(map) = arguments
        && map.len() == 1
        && let Some(serde_json::Value::String(command)) = map.get("command")
    {
        return format!("{name}\n  {command}");
    }
    let args = pretty_arguments(arguments);
    if args.is_empty() {
        name.to_string()
    } else {
        format!("{name} {args}")
    }
}

/// Render a tool result: its content indented, truncated if very long,
/// followed by a blank line.
fn render_result(result: &ToolResult, out: &mut impl Write) -> std::io::Result<()> {
    let lines: Vec<&str> = result.content.lines().collect();
    if lines.is_empty() {
        return writeln!(out);
    }
    let shown = if lines.len() <= RESULT_PREVIEW_LINES {
        lines.len()
    } else {
        RESULT_TRUNCATED_LINES
    };
    for line in &lines[..shown] {
        if line.is_empty() {
            writeln!(out)?;
        } else {
            writeln!(out, "  {line}")?;
        }
    }
    if lines.len() > shown {
        writeln!(out, "  … {} more lines", lines.len() - shown)?;
    }
    writeln!(out)
}

/// Compact, single-line rendering of tool call arguments.
fn pretty_arguments(arguments: &serde_json::Value) -> String {
    match arguments {
        serde_json::Value::Null => String::new(),
        serde_json::Value::Object(map) if map.is_empty() => String::new(),
        _ => serde_json::to_string(arguments).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vava_core::{ToolCall, ToolResult};

    fn render(events: &[AgentEvent], show_reasoning: bool) -> String {
        let mut renderer = Renderer::new(show_reasoning);
        let mut out = Vec::new();
        for event in events {
            renderer.render(event, &mut out).unwrap();
        }
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn renders_the_prompt_header() {
        let renderer = Renderer::new(false);
        let mut out = Vec::new();
        renderer.render_prompt("fix the tests", &mut out).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "> fix the tests\n\n");
    }

    #[test]
    fn streams_text_deltas() {
        let out = render(
            &[
                AgentEvent::TurnStarted,
                AgentEvent::TextDelta {
                    delta: "Hello ".into(),
                },
                AgentEvent::TextDelta {
                    delta: "world".into(),
                },
                AgentEvent::TurnCompleted,
            ],
            false,
        );
        assert_eq!(out, "Hello world\n");
    }

    #[test]
    fn shows_a_thinking_marker_once_by_default() {
        let out = render(
            &[
                AgentEvent::TurnStarted,
                AgentEvent::ReasoningDelta {
                    delta: "think one".into(),
                },
                AgentEvent::ReasoningDelta {
                    delta: " think two".into(),
                },
                AgentEvent::TextDelta {
                    delta: "answer".into(),
                },
            ],
            false,
        );
        assert_eq!(out, "Thinking...\n\nanswer");
    }

    #[test]
    fn streams_reasoning_in_debug_mode() {
        let out = render(
            &[AgentEvent::ReasoningDelta {
                delta: "secret".into(),
            }],
            true,
        );
        assert_eq!(out, "secret");
    }

    #[test]
    fn renders_bash_calls_with_the_command_on_its_own_line() {
        let out = render(
            &[
                AgentEvent::ToolCallStarted {
                    call: ToolCall {
                        id: "c1".into(),
                        name: "bash".into(),
                        arguments: serde_json::json!({"command": "cargo test"}),
                    },
                },
                AgentEvent::ToolCallFinished {
                    call_id: "c1".into(),
                    result: ToolResult::error("exit code 101"),
                },
            ],
            false,
        );
        assert_eq!(out, "● bash\n  cargo test\n\n  exit code 101\n\n");
    }

    #[test]
    fn renders_path_calls_inline() {
        let out = render(
            &[
                AgentEvent::ToolCallStarted {
                    call: ToolCall {
                        id: "c1".into(),
                        name: "read".into(),
                        arguments: serde_json::json!({"path": "src/lib.rs"}),
                    },
                },
                AgentEvent::ToolCallFinished {
                    call_id: "c1".into(),
                    result: ToolResult::ok("1 | fn main() {}\n"),
                },
            ],
            false,
        );
        assert_eq!(
            out,
            "● read {\"path\":\"src/lib.rs\"}\n\n  1 | fn main() {}\n\n"
        );
    }

    #[test]
    fn truncates_long_tool_results() {
        let long = (1..=20)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let out = render(
            &[AgentEvent::ToolCallFinished {
                call_id: "c1".into(),
                result: ToolResult::ok(&long),
            }],
            false,
        );
        assert!(out.contains("  line 1"));
        assert!(out.contains("  line 4"));
        assert!(out.contains("… 16 more lines"));
        assert!(!out.contains("line 20"));
    }

    #[test]
    fn prints_errors() {
        let out = render(
            &[AgentEvent::Error {
                message: "kaput".into(),
            }],
            false,
        );
        assert_eq!(out, "\nerror: kaput\n\n");
    }
}
