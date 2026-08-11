//! Assembling complete assistant messages from streaming model events.

use serde_json::Value;

use crate::Usage;
use crate::event::ModelEvent;
use crate::message::{AssistantMessage, ToolCall};

/// Accumulates [`ModelEvent`]s into a complete [`AssistantMessage`].
///
/// Text and reasoning deltas are concatenated as they arrive; tool-call
/// fragments are accumulated per index, and their arguments are deserialized
/// only once the whole response has streamed in.
#[derive(Debug, Default)]
pub struct AssistantBuilder {
    content: String,
    reasoning: String,
    tool_calls: Vec<PartialToolCall>,
    usage: Option<Usage>,
    finished: bool,
}

/// A tool call being assembled from stream fragments.
#[derive(Debug, Default)]
struct PartialToolCall {
    id: String,
    name: String,
    /// Raw JSON text; parsed only when the message is complete.
    arguments: String,
}

impl AssistantBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one model event.
    pub fn push(&mut self, event: ModelEvent) {
        match event {
            ModelEvent::TextDelta(delta) => self.content.push_str(&delta),
            ModelEvent::ReasoningDelta(delta) => self.reasoning.push_str(&delta),
            ModelEvent::ToolCallStarted { index, id, name } => {
                let call = self.tool_call_mut(index);
                call.id = id;
                call.name = name;
            }
            ModelEvent::ToolCallArgumentsDelta { index, delta } => {
                self.tool_call_mut(index).arguments.push_str(&delta);
            }
            ModelEvent::Usage(usage) => self.usage = Some(usage),
            ModelEvent::Finished => self.finished = true,
        }
    }

    fn tool_call_mut(&mut self, index: usize) -> &mut PartialToolCall {
        if self.tool_calls.len() <= index {
            self.tool_calls
                .resize_with(index + 1, PartialToolCall::default);
        }
        &mut self.tool_calls[index]
    }

    /// Whether the model signaled the end of this response.
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Token usage reported by the model, if any.
    pub fn usage(&self) -> Option<Usage> {
        self.usage
    }

    /// Assemble the complete assistant message.
    ///
    /// Tool calls whose name never arrived (a malformed stream) are dropped:
    /// an unnamed call cannot be executed, and sending one back to the model
    /// would corrupt the next request.
    pub fn build(self) -> AssistantMessage {
        let tool_calls = self
            .tool_calls
            .into_iter()
            .filter(|call| !call.name.is_empty())
            .map(|call| ToolCall {
                id: call.id,
                name: call.name,
                arguments: parse_arguments(&call.arguments),
            })
            .collect();

        AssistantMessage {
            content: self.content,
            reasoning_content: (!self.reasoning.is_empty()).then_some(self.reasoning),
            tool_calls,
        }
    }
}

/// Parse accumulated tool-call arguments. A malformed or incomplete JSON
/// fragment yields `Null`, which the tool layer rejects with a clear
/// `InvalidArguments` error the model can recover from.
fn parse_arguments(raw: &str) -> Value {
    if raw.is_empty() {
        return Value::Null;
    }
    serde_json::from_str(raw).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn assembles_text_and_reasoning() {
        let mut builder = AssistantBuilder::new();
        builder.push(ModelEvent::ReasoningDelta("think".into()));
        builder.push(ModelEvent::TextDelta("Hello ".into()));
        builder.push(ModelEvent::TextDelta("world".into()));
        builder.push(ModelEvent::Finished);
        assert!(builder.is_finished());
        let message = builder.build();
        assert_eq!(message.content, "Hello world");
        assert_eq!(message.reasoning_content.as_deref(), Some("think"));
        assert!(message.tool_calls.is_empty());
    }

    #[test]
    fn assembles_tool_calls_from_fragments() {
        let mut builder = AssistantBuilder::new();
        builder.push(ModelEvent::ToolCallStarted {
            index: 0,
            id: "call_1".into(),
            name: "read".into(),
        });
        builder.push(ModelEvent::ToolCallArgumentsDelta {
            index: 0,
            delta: r#"{"path":"src"#.into(),
        });
        builder.push(ModelEvent::ToolCallArgumentsDelta {
            index: 0,
            delta: r#"/main.rs"}"#.into(),
        });
        builder.push(ModelEvent::ToolCallStarted {
            index: 1,
            id: "call_2".into(),
            name: "bash".into(),
        });
        builder.push(ModelEvent::ToolCallArgumentsDelta {
            index: 1,
            delta: r#"{"command":"cargo test"}"#.into(),
        });
        builder.push(ModelEvent::Finished);
        let message = builder.build();
        assert_eq!(message.tool_calls.len(), 2);
        assert_eq!(message.tool_calls[0].id, "call_1");
        assert_eq!(message.tool_calls[0].name, "read");
        assert_eq!(
            message.tool_calls[0].arguments,
            json!({"path": "src/main.rs"})
        );
        assert_eq!(
            message.tool_calls[1].arguments,
            json!({"command": "cargo test"})
        );
    }

    #[test]
    fn malformed_arguments_become_null() {
        let mut builder = AssistantBuilder::new();
        builder.push(ModelEvent::ToolCallStarted {
            index: 0,
            id: "c".into(),
            name: "read".into(),
        });
        builder.push(ModelEvent::ToolCallArgumentsDelta {
            index: 0,
            delta: "{oops".into(),
        });
        builder.push(ModelEvent::Finished);
        let message = builder.build();
        assert_eq!(message.tool_calls[0].arguments, Value::Null);
    }

    #[test]
    fn keeps_reasoning_when_tool_calls_present() {
        // The DeepSeek requirement: reasoning must survive tool-call loops.
        let mut builder = AssistantBuilder::new();
        builder.push(ModelEvent::ReasoningDelta("let me check".into()));
        builder.push(ModelEvent::ToolCallStarted {
            index: 0,
            id: "c".into(),
            name: "bash".into(),
        });
        builder.push(ModelEvent::Finished);
        let message = builder.build();
        assert_eq!(message.reasoning_content.as_deref(), Some("let me check"));
        assert_eq!(message.tool_calls.len(), 1);
    }

    #[test]
    fn tracks_usage_and_finished() {
        let mut builder = AssistantBuilder::new();
        assert!(!builder.is_finished());
        builder.push(ModelEvent::Usage(Usage::new(10, 5)));
        builder.push(ModelEvent::Finished);
        assert!(builder.is_finished());
        assert_eq!(builder.usage(), Some(Usage::new(10, 5)));
    }

    #[test]
    fn unnamed_calls_are_dropped() {
        // Out-of-order indices get padded; calls with no name are dropped.
        let mut builder = AssistantBuilder::new();
        builder.push(ModelEvent::ToolCallStarted {
            index: 2,
            id: "c".into(),
            name: "read".into(),
        });
        builder.push(ModelEvent::Finished);
        let message = builder.build();
        assert_eq!(message.tool_calls.len(), 1);
        assert_eq!(message.tool_calls[0].name, "read");
    }
}
