//! The seam between the agent harness and whatever model it talks to.
//!
//! `vava-core` cannot depend on `vava-deepseek` — the dependency direction
//! is the other way — and the agent loop must be testable without any API.
//! So the harness talks to a [`ModelClient`], a single small trait with one
//! method. `DeepSeekClient` implements it in `vava-deepseek`; tests
//! implement it with scripted fakes.
//!
//! This is the only provider-related seam in the core, and it exists because
//! fake-model tests are a requirement, not because vava supports multiple
//! providers.

use std::error::Error;

use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::event::ModelEvent;
use crate::message::Message;
use crate::tool::ToolDefinition;

/// An error erased across the client seam. Errors keep their full type
/// inside each provider crate; only the boundary erases them.
pub type BoxedError = Box<dyn Error + Send + Sync>;

/// Something that can stream a model response for a conversation.
#[async_trait]
pub trait ModelClient: Send + Sync {
    /// Stream the model's next response for the given transcript.
    ///
    /// The system prompt is passed separately from the transcript; the
    /// client decides how to present it to its API.
    async fn stream(
        &self,
        messages: &[Message],
        system: &str,
        tools: &[ToolDefinition],
    ) -> Result<BoxStream<'static, Result<ModelEvent, BoxedError>>, BoxedError>;
}
