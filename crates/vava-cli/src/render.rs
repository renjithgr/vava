//! Rendering of [`AgentEvent`]s for the print CLI.
//!
//! This is the simplest frontend: text streaming to stdout. The REPL and the
//! Ratatui TUI will render the same events differently — the events are the
//! contract, not the rendering.

use std::io::Write;

use tokio::sync::mpsc;

use vava_core::AgentEvent;

/// Renders agent events to a text stream.
pub struct Renderer {
    /// Whether reasoning deltas are shown (the `--debug` flag).
    show_reasoning: bool,
}

impl Renderer {
    pub fn new(show_reasoning: bool) -> Self {
        Self { show_reasoning }
    }

    /// Render one event.
    pub fn render(&mut self, event: &AgentEvent, out: &mut impl Write) -> std::io::Result<()> {
        match event {
            AgentEvent::TurnStarted => writeln!(out),
            AgentEvent::TextDelta { delta } => write!(out, "{delta}"),
            AgentEvent::ReasoningDelta { delta } => {
                if self.show_reasoning {
                    write!(out, "{delta}")
                } else {
                    Ok(())
                }
            }
            AgentEvent::ToolCallStarted { call } => {
                writeln!(
                    out,
                    "\n● {} {}",
                    call.name,
                    pretty_arguments(&call.arguments)
                )
            }
            AgentEvent::ToolCallFinished { result, .. } => {
                let first_line = result.content.lines().next().unwrap_or_default();
                if result.is_error {
                    writeln!(out, "  {first_line}")?;
                    writeln!(out, "  (tool reported an error)")
                } else {
                    writeln!(out, "  {first_line}")
                }
            }
            AgentEvent::AssistantMessageCompleted { .. } => Ok(()),
            AgentEvent::TurnCompleted => writeln!(out, "\n"),
            AgentEvent::Error { message } => writeln!(out, "\nerror: {message}"),
        }
    }
}

/// Render the event stream to stdout until the channel closes.
pub async fn render_events(mut rx: mpsc::Receiver<AgentEvent>, show_reasoning: bool) {
    let mut renderer = Renderer::new(show_reasoning);
    while let Some(event) = rx.recv().await {
        // The stdout lock is taken per event so it is never held across an
        // await point (which would make this future !Send).
        let stdout = std::io::stdout();
        let mut out = stdout.lock();
        let _ = renderer.render(&event, &mut out);
        let _ = out.flush();
    }
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
    use vava_core::{AgentEvent, ToolCall, ToolResult};

    fn render(events: &[AgentEvent], show_reasoning: bool) -> String {
        let mut renderer = Renderer::new(show_reasoning);
        let mut out = Vec::new();
        for event in events {
            renderer.render(event, &mut out).unwrap();
        }
        String::from_utf8(out).unwrap()
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
        assert_eq!(out, "\nHello world\n\n");
    }

    #[test]
    fn hides_reasoning_by_default_shows_with_debug() {
        let events = [AgentEvent::ReasoningDelta {
            delta: "secret".into(),
        }];
        assert_eq!(render(&events, false), "");
        assert_eq!(render(&events, true), "secret");
    }

    #[test]
    fn prints_tool_calls_and_results() {
        let out = render(
            &[
                AgentEvent::ToolCallStarted {
                    call: ToolCall::new("c1", "bash"),
                },
                AgentEvent::ToolCallFinished {
                    call_id: "c1".into(),
                    result: ToolResult::error("exit code 101\n\nmore output"),
                },
                AgentEvent::ToolCallFinished {
                    call_id: "c2".into(),
                    result: ToolResult::ok("all good"),
                },
            ],
            false,
        );
        assert!(out.contains("● bash"));
        assert!(out.contains("exit code 101"));
        assert!(out.contains("(tool reported an error)"));
        assert!(out.contains("all good"));
    }

    #[test]
    fn prints_errors() {
        let out = render(
            &[AgentEvent::Error {
                message: "kaput".into(),
            }],
            false,
        );
        assert_eq!(out, "\nerror: kaput\n");
    }
}
