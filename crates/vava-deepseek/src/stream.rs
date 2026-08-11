//! SSE streaming support: framing, payload interpretation, and the
//! translation of stream chunks into [`ModelEvent`]s.
//!
//! Two small layers, both shaped by the DeepSeek (OpenAI-compatible) stream
//! format:
//!
//! 1. [`SseParser`] — incremental framing. Feed raw bytes as they arrive
//!    over the wire; get complete `data:` payloads back. Handles `\n` and
//!    `\r\n` line endings, multi-line data fields, comments, and byte
//!    streams split at arbitrary boundaries.
//! 2. [`parse_payload`] — interpret one payload as a [`StreamChunk`] or the
//!    end-of-stream `[DONE]` marker.
//!
//! [`ChunkTranslator`] then turns chunks into [`ModelEvent`]s, emitting text
//! and reasoning deltas immediately and tracking tool calls by index, so the
//! agent layer can accumulate fragments and deserialize arguments only once
//! a call is complete.

use std::collections::HashSet;

use vava_core::{ModelEvent, Usage};

use crate::error::DeepSeekError;
use crate::response::{DeepSeekUsage, StreamChunk};

/// Incremental SSE framing parser.
///
/// ```text
/// raw bytes ──► SseParser ──► complete `data:` payloads
/// ```
///
/// Infallible framing is a deliberate choice: the parser is lenient about
/// the bytes it does not care about (comments, `event:`/`id:`/`retry:`
/// fields) and strict only about the `data:` fields it must deliver.
pub struct SseParser {
    /// Bytes received but not yet assembled into complete lines.
    buffer: Vec<u8>,
    /// The `data:` payload currently being assembled. Consecutive `data:`
    /// lines within one event block are joined with `\n`, per the SSE spec.
    current: Option<String>,
    /// Whether the current event block has seen at least one `data:` line.
    saw_data: bool,
}

impl Default for SseParser {
    fn default() -> Self {
        Self::new()
    }
}

impl SseParser {
    pub fn new() -> Self {
        Self {
            buffer: Vec::new(),
            current: None,
            saw_data: false,
        }
    }

    /// Feed a chunk of raw bytes, returning any complete `data:` payloads.
    ///
    /// The byte stream may be split at any boundary — including inside a
    /// multi-byte UTF-8 character or a JSON token.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>, DeepSeekError> {
        self.buffer.extend_from_slice(bytes);
        let mut payloads = Vec::new();
        while let Some(end) = self.buffer.iter().position(|&b| b == b'\n') {
            let mut line: Vec<u8> = self.buffer.drain(..=end).collect();
            line.pop(); // the '\n'
            if line.last() == Some(&b'\r') {
                line.pop(); // CRLF
            }
            self.process_line(&line, &mut payloads)?;
        }
        Ok(payloads)
    }

    /// Deliver any trailing payload not terminated by a blank line. Call
    /// once after the underlying stream ends, so a truncated final event is
    /// still surfaced.
    pub fn finish(&mut self) -> Option<String> {
        if self.saw_data {
            self.saw_data = false;
            self.current.take()
        } else {
            None
        }
    }

    fn process_line(&mut self, line: &[u8], out: &mut Vec<String>) -> Result<(), DeepSeekError> {
        if line.is_empty() {
            // A blank line ends the event block: dispatch the data payload.
            if self.saw_data {
                if let Some(payload) = self.current.take() {
                    out.push(payload);
                }
                self.saw_data = false;
            }
            return Ok(());
        }

        // `:...` lines are comments and must be ignored.
        if line.starts_with(b":") {
            return Ok(());
        }

        if let Some(rest) = line.strip_prefix(b"data:") {
            // `data:value` and `data: value` are both valid.
            let value = rest.strip_prefix(b" ").unwrap_or(rest);
            let value = String::from_utf8(value.to_vec())
                .map_err(|_| DeepSeekError::Sse("data field is not valid UTF-8".into()))?;
            if self.saw_data {
                // Consecutive data lines are joined with a newline.
                if let Some(current) = &mut self.current {
                    current.push('\n');
                    current.push_str(&value);
                }
            } else {
                self.current = Some(value);
            }
            self.saw_data = true;
            return Ok(());
        }

        // Other fields (`event:`, `id:`, `retry:`) are not needed by vava.
        Ok(())
    }
}

/// One `data:` payload from the stream, interpreted.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    /// A regular streaming chunk.
    Chunk(StreamChunk),
    /// The end-of-stream marker `[DONE]`.
    Done,
}

/// Interpret a single `data:` payload.
pub fn parse_payload(payload: &str) -> Result<StreamEvent, DeepSeekError> {
    if payload == "[DONE]" {
        return Ok(StreamEvent::Done);
    }
    let chunk: StreamChunk = serde_json::from_str(payload)
        .map_err(|e| DeepSeekError::Protocol(format!("invalid chunk JSON: {e}")))?;
    Ok(StreamEvent::Chunk(chunk))
}

/// Translates [`StreamChunk`]s into [`ModelEvent`]s.
///
/// Text and reasoning deltas pass through immediately. Tool calls are
/// tracked by their index so that `ToolCallStarted` is emitted exactly once
/// per call, while `ToolCallArgumentsDelta` events carry each fragment for
/// the agent layer to accumulate. Token usage and the `Finished` marker are
/// forwarded when the stream reports them.
pub struct ChunkTranslator {
    /// Indices of tool calls for which `ToolCallStarted` was already emitted.
    started: HashSet<usize>,
}

impl Default for ChunkTranslator {
    fn default() -> Self {
        Self::new()
    }
}

impl ChunkTranslator {
    pub fn new() -> Self {
        Self {
            started: HashSet::new(),
        }
    }

    /// Translate one chunk into zero or more model events.
    pub fn feed(&mut self, chunk: &StreamChunk) -> Vec<ModelEvent> {
        let mut events = Vec::new();

        for choice in &chunk.choices {
            let delta = &choice.delta;

            if let Some(content) = &delta.content
                && !content.is_empty()
            {
                events.push(ModelEvent::TextDelta(content.clone()));
            }

            if let Some(reasoning) = &delta.reasoning_content
                && !reasoning.is_empty()
            {
                events.push(ModelEvent::ReasoningDelta(reasoning.clone()));
            }

            for call in &delta.tool_calls {
                let index = call.index;
                if self.started.insert(index) {
                    events.push(ModelEvent::ToolCallStarted {
                        index,
                        id: call.id.clone().unwrap_or_default(),
                        name: call
                            .function
                            .as_ref()
                            .and_then(|function| function.name.clone())
                            .unwrap_or_default(),
                    });
                }
                if let Some(function) = &call.function
                    && let Some(arguments) = &function.arguments
                    && !arguments.is_empty()
                {
                    events.push(ModelEvent::ToolCallArgumentsDelta {
                        index,
                        delta: arguments.clone(),
                    });
                }
            }
        }

        if let Some(usage) = chunk.usage {
            events.push(ModelEvent::Usage(translate_usage(usage)));
        }

        if chunk
            .choices
            .iter()
            .any(|choice| choice.finish_reason.is_some())
        {
            events.push(ModelEvent::Finished);
        }

        events
    }
}

fn translate_usage(usage: DeepSeekUsage) -> Usage {
    Usage {
        prompt_tokens: usage.prompt_tokens,
        completion_tokens: usage.completion_tokens,
        total_tokens: usage.total_tokens,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ---------------------------------------------------------------- framing

    #[test]
    fn framing_handles_split_bytes_and_crlf() {
        // The stream below is deliberately fed one byte at a time, and uses
        // CRLF line endings, to exercise both fragmentation and \r\n.
        let stream = "data: {\"id\":\"x\",\"choices\":[]}\r\n\r\ndata: [DONE]\r\n\r\n";
        let mut parser = SseParser::new();
        let mut payloads = Vec::new();
        for byte in stream.as_bytes() {
            payloads.extend(parser.push(std::slice::from_ref(byte)).unwrap());
        }
        assert_eq!(parser.finish(), None);
        assert_eq!(payloads, vec![r#"{"id":"x","choices":[]}"#, "[DONE]"]);
    }

    #[test]
    fn framing_joins_multi_line_data_with_newline() {
        let stream = "data: first\ndata: second\n\n";
        let mut parser = SseParser::new();
        let payloads = parser.push(stream.as_bytes()).unwrap();
        assert_eq!(payloads, vec!["first\nsecond"]);
    }

    #[test]
    fn framing_ignores_comments_and_other_fields() {
        let stream = ": keep-alive comment\nid: 1\nevent: message\ndata: {\"id\":\"x\"}\n\n";
        let mut parser = SseParser::new();
        let payloads = parser.push(stream.as_bytes()).unwrap();
        assert_eq!(payloads, vec![r#"{"id":"x"}"#]);
    }

    #[test]
    fn framing_tolerates_missing_trailing_blank_line() {
        let stream = "data: {\"id\":\"x\"}\n";
        let mut parser = SseParser::new();
        let payloads = parser.push(stream.as_bytes()).unwrap();
        assert!(payloads.is_empty());
        assert_eq!(parser.finish().as_deref(), Some(r#"{"id":"x"}"#));
    }

    #[test]
    fn framing_rejects_invalid_utf8_in_data() {
        let mut parser = SseParser::new();
        let err = parser.push(b"data: \xff\xfe\n\n").unwrap_err();
        assert!(matches!(err, DeepSeekError::Sse(_)));
    }

    // ---------------------------------------------------------- payloads

    #[test]
    fn parse_payload_recognizes_done() {
        assert_eq!(parse_payload("[DONE]").unwrap(), StreamEvent::Done);
    }

    #[test]
    fn parse_payload_accepts_a_chunk() {
        let payload =
            r#"{"id":"x","choices":[{"index":0,"delta":{"content":"hi"},"finish_reason":null}]}"#;
        let event = parse_payload(payload).unwrap();
        match event {
            StreamEvent::Chunk(chunk) => {
                assert_eq!(chunk.id, "x");
                assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("hi"));
            }
            StreamEvent::Done => panic!("expected a chunk"),
        }
    }

    #[test]
    fn parse_payload_rejects_invalid_json() {
        let err = parse_payload("{not json").unwrap_err();
        assert!(matches!(err, DeepSeekError::Protocol(_)));
    }

    #[test]
    fn parse_payload_rejects_json_that_is_not_a_chunk() {
        // Valid JSON, but not a StreamChunk (missing required fields).
        let err = parse_payload(r#"{"foo": 1}"#).unwrap_err();
        assert!(matches!(err, DeepSeekError::Protocol(_)));
    }

    // --------------------------------------------------------- translator

    fn chunk(json: serde_json::Value) -> StreamChunk {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn translator_emits_start_once_and_accumulates_arguments() {
        let mut translator = ChunkTranslator::new();

        let events = translator.feed(&chunk(json!({
            "id": "x",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": "bash", "arguments": ""}
                    }]
                },
                "finish_reason": null
            }]
        })));
        assert_eq!(
            events,
            vec![ModelEvent::ToolCallStarted {
                index: 0,
                id: "call_1".into(),
                name: "bash".into(),
            }]
        );

        let events = translator.feed(&chunk(json!({
            "id": "x",
            "choices": [{
                "index": 0,
                "delta": {"tool_calls": [{"index": 0, "function": {"arguments": "cargo "}}]},
                "finish_reason": null
            }]
        })));
        assert_eq!(
            events,
            vec![ModelEvent::ToolCallArgumentsDelta {
                index: 0,
                delta: "cargo ".into(),
            }]
        );

        // A second call on the same chunk must not re-emit ToolCallStarted.
        let events = translator.feed(&chunk(json!({
            "id": "x",
            "choices": [{
                "index": 0,
                "delta": {"tool_calls": [{"index": 1, "id": "call_2", "type": "function", "function": {"name": "read", "arguments": "{\"path\":"}}]},
                "finish_reason": null
            }]
        })));
        assert_eq!(
            events,
            vec![
                ModelEvent::ToolCallStarted {
                    index: 1,
                    id: "call_2".into(),
                    name: "read".into(),
                },
                ModelEvent::ToolCallArgumentsDelta {
                    index: 1,
                    delta: r#"{"path":"#.into(),
                },
            ]
        );
    }

    #[test]
    fn translator_forwards_text_and_reasoning_deltas() {
        let mut translator = ChunkTranslator::new();
        let events = translator.feed(&chunk(json!({
            "id": "x",
            "choices": [{
                "index": 0,
                "delta": {"reasoning_content": "think", "content": "Hello"},
                "finish_reason": null
            }]
        })));
        assert_eq!(
            events,
            vec![
                ModelEvent::TextDelta("Hello".into()),
                ModelEvent::ReasoningDelta("think".into()),
            ]
        );
    }

    #[test]
    fn translator_skips_empty_deltas() {
        let mut translator = ChunkTranslator::new();
        let events = translator.feed(&chunk(json!({
            "id": "x",
            "choices": [{
                "index": 0,
                "delta": {"role": "assistant", "content": "", "reasoning_content": ""},
                "finish_reason": null
            }]
        })));
        assert!(events.is_empty());
    }

    #[test]
    fn translator_emits_finished_and_usage() {
        let mut translator = ChunkTranslator::new();
        let events = translator.feed(&chunk(json!({
            "id": "x",
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15,
                "prompt_cache_hit_tokens": 0,
                "prompt_cache_miss_tokens": 10
            }
        })));
        assert_eq!(
            events,
            vec![ModelEvent::Usage(Usage::new(10, 5)), ModelEvent::Finished,]
        );
    }
}
