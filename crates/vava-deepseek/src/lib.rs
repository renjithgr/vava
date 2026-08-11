//! vava-deepseek — the DeepSeek API client for vava.
//!
//! This crate knows everything about the DeepSeek wire protocol and nothing
//! about repositories, tools, or terminals:
//!
//! - [`request`] — build and serialize DeepSeek requests from
//!   `vava-core` messages (the explicit DeepSeek serializer)
//! - [`response`] — the response shapes: chat completions, streaming
//!   chunks, usage, and API error bodies
//! - [`stream`] — the SSE parser: framing, payload interpretation, and the
//!   translation of chunks into [`vava_core::ModelEvent`]s
//! - [`client`] — the HTTP client: sends requests, consumes SSE, emits
//!   model events, implements [`vava_core::ModelClient`]
//! - [`error`] — typed errors raised by this layer
//! - [`model`] — client configuration (model, thinking mode, base URL)
//!
//! It never executes tools, never touches the filesystem, and never prints.

pub mod client;
pub mod error;
pub mod model;
pub mod request;
pub mod response;
pub mod stream;

pub use client::DeepSeekClient;
pub use error::DeepSeekError;
pub use model::{DEFAULT_BASE_URL, DEFAULT_MODEL, ModelConfig};
pub use request::{DeepSeekRequest, Role, Thinking, ThinkingMode, WireMessage, WireTool};
pub use response::{ApiErrorBody, ChatResponse, DeepSeekUsage, StreamChunk};
pub use stream::{ChunkTranslator, SseParser, StreamEvent, parse_payload};
