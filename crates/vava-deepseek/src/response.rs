//! DeepSeek response types.
//!
//! These mirror the DeepSeek wire shapes and are parsed from JSON by the
//! SSE parser (next milestone) and the client. Nothing here knows about
//! conversations or repositories — the conversion from wire shapes to
//! `vava-core` types happens in the streaming/assembling code.

use serde::Deserialize;

/// A complete (non-streaming) chat completion response.
#[derive(Debug, Clone, Deserialize)]
pub struct ChatResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChatChoice>,
    pub usage: Option<DeepSeekUsage>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatChoice {
    pub index: usize,
    pub message: ResponseMessage,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ResponseMessage {
    pub role: Option<String>,
    /// `None` when the message only carries tool calls.
    pub content: Option<String>,
    /// DeepSeek-specific: the assistant's reasoning text.
    pub reasoning_content: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<ResponseToolCall>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ResponseToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub function: ResponseFunction,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ResponseFunction {
    pub name: String,
    /// The arguments as a JSON-encoded *string*, e.g.
    /// `{"path":"src/main.rs"}`.
    pub arguments: String,
}

/// One SSE chunk of a streaming response (`chat.completion.chunk`).
#[derive(Debug, Clone, Deserialize)]
pub struct StreamChunk {
    pub id: String,
    pub object: Option<String>,
    pub created: Option<u64>,
    pub model: Option<String>,
    #[serde(default)]
    pub choices: Vec<StreamChoice>,
    /// Usage usually arrives on the final chunk(s), with no choices.
    pub usage: Option<DeepSeekUsage>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StreamChoice {
    pub index: usize,
    pub delta: Delta,
    /// `"stop"`, `"tool_calls"`, `"length"`, ... — `None` while streaming.
    pub finish_reason: Option<String>,
}

/// The `delta` of a streaming choice: the piece of the message that arrived
/// in this chunk.
#[derive(Debug, Clone, Deserialize)]
pub struct Delta {
    pub role: Option<String>,
    pub content: Option<String>,
    pub reasoning_content: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<DeltaToolCall>,
}

/// A fragment of a tool call inside a delta. `arguments` may be split across
/// many chunks and must be accumulated per `index`.
#[derive(Debug, Clone, Deserialize)]
pub struct DeltaToolCall {
    /// Which tool call (within this message) this fragment belongs to.
    pub index: usize,
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub function: Option<DeltaFunction>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeltaFunction {
    pub name: Option<String>,
    pub arguments: Option<String>,
}

/// Token usage reported by DeepSeek.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct DeepSeekUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub prompt_cache_hit_tokens: Option<u64>,
    pub prompt_cache_miss_tokens: Option<u64>,
}

/// The JSON error body returned for HTTP 4xx/5xx responses.
#[derive(Debug, Clone, Deserialize)]
pub struct ApiErrorBody {
    pub error: ApiErrorDetail,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiErrorDetail {
    pub message: String,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub code: Option<String>,
    pub param: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_non_streaming_response_with_reasoning_and_tool_calls() {
        let json = r#"{
            "id": "chatcmpl-1",
            "object": "chat.completion",
            "created": 1700000000,
            "model": "deepseek-chat",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": null,
                    "reasoning_content": "think",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": "read", "arguments": "{\"path\":\"x\"}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15,
                "prompt_cache_hit_tokens": 2,
                "prompt_cache_miss_tokens": 8
            }
        }"#;
        let response: ChatResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.id, "chatcmpl-1");
        let choice = &response.choices[0];
        assert_eq!(choice.finish_reason.as_deref(), Some("tool_calls"));
        assert_eq!(choice.message.content, None);
        assert_eq!(choice.message.reasoning_content.as_deref(), Some("think"));
        let call = &choice.message.tool_calls[0];
        assert_eq!(call.id, "call_1");
        assert_eq!(call.function.name, "read");
        assert_eq!(call.function.arguments, r#"{"path":"x"}"#);
        let usage = response.usage.unwrap();
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.total_tokens, 15);
        assert_eq!(usage.prompt_cache_hit_tokens, Some(2));
        assert_eq!(usage.prompt_cache_miss_tokens, Some(8));
    }

    #[test]
    fn parses_stream_chunk_with_content_delta() {
        let chunk: StreamChunk = serde_json::from_str(
            r#"{"id":"x","object":"chat.completion.chunk","created":1,"model":"m","choices":[{"index":0,"delta":{"role":"assistant","content":"Hel"},"finish_reason":null}]}"#,
        )
        .unwrap();
        assert_eq!(chunk.id, "x");
        assert_eq!(chunk.choices.len(), 1);
        let delta = &chunk.choices[0].delta;
        assert_eq!(delta.content.as_deref(), Some("Hel"));
        assert_eq!(delta.reasoning_content, None);
        assert!(delta.tool_calls.is_empty());
    }

    #[test]
    fn parses_stream_chunk_with_reasoning_delta() {
        let chunk: StreamChunk = serde_json::from_str(
            r#"{"id":"x","choices":[{"index":0,"delta":{"reasoning_content":"think"},"finish_reason":null}]}"#,
        )
        .unwrap();
        let delta = &chunk.choices[0].delta;
        assert_eq!(delta.reasoning_content.as_deref(), Some("think"));
        assert_eq!(delta.content, None);
    }

    #[test]
    fn accumulates_fragmented_tool_call_arguments_by_index() {
        let chunk1: StreamChunk = serde_json::from_str(
            r#"{"id":"x","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"read","arguments":"{\"path\":"}}]},"finish_reason":null}]}"#,
        )
        .unwrap();
        let chunk2: StreamChunk = serde_json::from_str(
            r#"{"id":"x","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"src/main.rs\"}"}}]},"finish_reason":null}]}"#,
        )
        .unwrap();
        let chunk3: StreamChunk = serde_json::from_str(
            r#"{"id":"x","choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"function":{"arguments":"{\"other\":1}"}}]},"finish_reason":null}]}"#,
        )
        .unwrap();

        // Simulate the accumulation the stream assembler will perform:
        // fragments are keyed by tool-call index, in order.
        let mut by_index: std::collections::BTreeMap<usize, String> = Default::default();
        for chunk in [chunk1, chunk2, chunk3] {
            for choice in chunk.choices {
                for call in choice.delta.tool_calls {
                    if let Some(function) = call.function
                        && let Some(arguments) = function.arguments
                    {
                        by_index.entry(call.index).or_default().push_str(&arguments);
                    }
                }
            }
        }

        assert_eq!(by_index.len(), 2);
        assert_eq!(by_index[&0], r#"{"path":"src/main.rs"}"#);
        assert_eq!(by_index[&1], r#"{"other":1}"#);

        // The accumulated string for index 0 parses as JSON.
        let parsed: serde_json::Value = serde_json::from_str(&by_index[&0]).unwrap();
        assert_eq!(parsed["path"], "src/main.rs");
    }

    #[test]
    fn parses_usage_chunk() {
        let chunk: StreamChunk = serde_json::from_str(
            r#"{"id":"x","choices":[],"usage":{"prompt_tokens":1,"completion_tokens":2,"total_tokens":3,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":1}}"#,
        )
        .unwrap();
        assert!(chunk.choices.is_empty());
        let usage = chunk.usage.unwrap();
        assert_eq!(usage.total_tokens, 3);
        assert_eq!(usage.prompt_cache_hit_tokens, Some(0));
    }

    #[test]
    fn parses_finish_reason_chunk() {
        let chunk: StreamChunk = serde_json::from_str(
            r#"{"id":"x","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
        )
        .unwrap();
        assert_eq!(chunk.choices[0].finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn parses_api_error_body() {
        let body: ApiErrorBody = serde_json::from_str(
            r#"{"error":{"message":"Invalid API key","type":"authentication_error","code":"invalid_api_key","param":null}}"#,
        )
        .unwrap();
        assert_eq!(body.error.message, "Invalid API key");
        assert_eq!(body.error.kind.as_deref(), Some("authentication_error"));
        assert_eq!(body.error.code.as_deref(), Some("invalid_api_key"));
        assert_eq!(body.error.param, None);
    }
}
