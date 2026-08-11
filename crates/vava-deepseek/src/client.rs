//! The DeepSeek HTTP client.
//!
//! This is the only place that talks to the DeepSeek API. It constructs the
//! request, sends it, and turns the SSE response into a stream of
//! [`ModelEvent`]s. It knows nothing about repositories, tools, or
//! terminals.

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use reqwest::header::AUTHORIZATION;
use secrecy::{ExposeSecret, SecretString};

use vava_core::{Message, ModelClient, ModelEvent, ToolDefinition};

use crate::error::DeepSeekError;
use crate::model::ModelConfig;
use crate::request::{DeepSeekRequest, Thinking};
use crate::response::ApiErrorBody;
use crate::stream::{ChunkTranslator, SseParser, StreamEvent, parse_payload};

/// A minimal, auditable DeepSeek API client.
///
/// Construct it with the API key and a [`ModelConfig`]; then call [`stream`]
/// (or use it as a [`ModelClient`]).
///
/// [`stream`]: DeepSeekClient::stream
pub struct DeepSeekClient {
    http: reqwest::Client,
    api_key: SecretString,
    config: ModelConfig,
}

impl std::fmt::Debug for DeepSeekClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Deliberately omit the API key. (SecretString already redacts its
        // own Debug output, but being explicit here costs nothing.)
        f.debug_struct("DeepSeekClient")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl DeepSeekClient {
    pub fn new(api_key: SecretString, config: ModelConfig) -> Self {
        Self {
            http: reqwest::Client::new(),
            api_key,
            config,
        }
    }

    /// The chat completions endpoint for this client's base URL.
    fn endpoint(&self) -> String {
        format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        )
    }

    /// Stream a model response for a conversation.
    ///
    /// The returned stream yields [`ModelEvent`]s as they arrive; when the
    /// underlying connection ends without a `Finished` event, one is emitted
    /// so consumers can always finalize the message.
    pub async fn stream(
        &self,
        messages: &[Message],
        system: &str,
        tools: &[ToolDefinition],
    ) -> Result<BoxStream<'static, Result<ModelEvent, DeepSeekError>>, DeepSeekError> {
        let thinking = self.config.thinking.then_some(Thinking::ENABLED);
        let request =
            DeepSeekRequest::chat(&self.config.model, system, messages, tools, true, thinking)
                .map_err(|error| {
                    DeepSeekError::Protocol(format!("could not serialize request: {error}"))
                })?;

        let response = self
            .http
            .post(self.endpoint())
            .header(
                AUTHORIZATION,
                format!("Bearer {}", self.api_key.expose_secret()),
            )
            .json(&request)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let message = response
                .json::<ApiErrorBody>()
                .await
                .map(|body| body.error.message)
                .unwrap_or_else(|_| format!("HTTP {status}"));
            return Err(DeepSeekError::Api {
                status: status.as_u16(),
                message,
            });
        }

        Ok(Box::pin(stream_events(response)))
    }
}

#[async_trait]
impl ModelClient for DeepSeekClient {
    async fn stream(
        &self,
        messages: &[Message],
        system: &str,
        tools: &[ToolDefinition],
    ) -> Result<BoxStream<'static, Result<ModelEvent, vava_core::BoxedError>>, vava_core::BoxedError>
    {
        let stream = self.stream(messages, system, tools).await?;
        Ok(Box::pin(stream.map(|item| item.map_err(Into::into))))
    }
}

/// Translate the SSE response body into a stream of model events.
fn stream_events(
    response: reqwest::Response,
) -> impl futures::Stream<Item = Result<ModelEvent, DeepSeekError>> {
    async_stream::try_stream! {
        let mut parser = SseParser::new();
        let mut translator = ChunkTranslator::new();
        let mut saw_finished = false;
        let mut bytes = response.bytes_stream();

        while let Some(chunk) = bytes.next().await {
            let chunk = chunk?;
            for payload in parser.push(&chunk)? {
                for event in events_for_payload(&payload, &mut translator)? {
                    saw_finished |= matches!(event, ModelEvent::Finished);
                    yield event;
                }
            }
        }

        // A truncated final event (no trailing blank line) is still data.
        if let Some(payload) = parser.finish() {
            for event in events_for_payload(&payload, &mut translator)? {
                saw_finished |= matches!(event, ModelEvent::Finished);
                yield event;
            }
        }

        // Always signal the end of the response.
        if !saw_finished {
            yield ModelEvent::Finished;
        }
    }
}

/// Interpret one SSE payload through the translator.
fn events_for_payload(
    payload: &str,
    translator: &mut ChunkTranslator,
) -> Result<Vec<ModelEvent>, DeepSeekError> {
    match parse_payload(payload)? {
        StreamEvent::Chunk(chunk) => Ok(translator.feed(&chunk)),
        StreamEvent::Done => Ok(Vec::new()),
    }
}
