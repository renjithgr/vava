//! Fixture-based tests for the SSE stream pipeline.
//!
//! The fixtures are recorded DeepSeek SSE streams. They are fed through the
//! framing parser in small byte chunks (simulating network fragmentation),
//! interpreted, and translated into [`ModelEvent`]s, which are then asserted
//! on exactly.

use vava_core::{ModelEvent, Usage};
use vava_deepseek::stream::{ChunkTranslator, SseParser, StreamEvent, parse_payload};

const TOOL_TURN: &str = include_str!("fixtures/tool_turn.stream.txt");
const FINAL_ANSWER: &str = include_str!("fixtures/final_answer.stream.txt");

/// Feed a whole stream through the framing parser in `chunk_size`-byte
/// pieces, collecting every `data:` payload in order.
fn frame(stream: &str, chunk_size: usize) -> Vec<String> {
    let mut parser = SseParser::new();
    let mut payloads = Vec::new();
    for piece in stream.as_bytes().chunks(chunk_size) {
        payloads.extend(parser.push(piece).unwrap());
    }
    if let Some(tail) = parser.finish() {
        payloads.push(tail);
    }
    payloads
}

/// Interpret payloads and translate chunks into model events.
fn to_events(payloads: &[String]) -> Vec<ModelEvent> {
    let mut translator = ChunkTranslator::new();
    let mut events = Vec::new();
    for payload in payloads {
        match parse_payload(payload).unwrap() {
            StreamEvent::Chunk(chunk) => events.extend(translator.feed(&chunk)),
            StreamEvent::Done => {}
        }
    }
    events
}

#[test]
fn tool_turn_frames_to_expected_payloads() {
    let payloads = frame(TOOL_TURN, 64);
    assert_eq!(payloads.len(), 8);
    assert!(payloads[0].starts_with(r#"{"id":"chatcmpl-tool""#));
    assert!(payloads[6].contains(r#""usage""#));
    assert_eq!(payloads[7], "[DONE]");
}

#[test]
fn tool_turn_produces_reasoning_tool_call_and_usage_events() {
    let events = to_events(&frame(TOOL_TURN, 64));
    assert_eq!(
        events,
        vec![
            ModelEvent::ReasoningDelta("Let me think about this.".into()),
            ModelEvent::ReasoningDelta(" First, I'll run the tests.".into()),
            ModelEvent::ToolCallStarted {
                index: 0,
                id: "call_1".into(),
                name: "bash".into(),
            },
            ModelEvent::ToolCallArgumentsDelta {
                index: 0,
                delta: "cargo test".into(),
            },
            ModelEvent::Finished,
            ModelEvent::Usage(Usage::new(20, 10)),
        ]
    );
}

#[test]
fn final_answer_concatenates_reasoning_and_text() {
    let events = to_events(&frame(FINAL_ANSWER, 64));

    // Fold the deltas the way the agent's assembler will.
    let mut reasoning = String::new();
    let mut content = String::new();
    for event in &events {
        match event {
            ModelEvent::ReasoningDelta(delta) => reasoning.push_str(delta),
            ModelEvent::TextDelta(delta) => content.push_str(delta),
            _ => {}
        }
    }
    assert_eq!(reasoning, "Done.");
    assert_eq!(content, "The tests pass now.");

    assert!(events.contains(&ModelEvent::Finished));
    assert!(!events.iter().any(|e| matches!(e, ModelEvent::Usage(_))));
}

/// The pipeline result must not depend on how the byte stream is fragmented.
#[test]
fn fragmentation_does_not_change_the_result() {
    let events = [3usize, 16, 64, 1024].map(|size| to_events(&frame(TOOL_TURN, size)));
    assert!(events.windows(2).all(|pair| pair[0] == pair[1]));
}
