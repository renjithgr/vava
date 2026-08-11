//! vava-deepseek — the DeepSeek API client for vava.
//!
//! This crate knows everything about the DeepSeek wire protocol and nothing
//! about repositories, tools, or terminals:
//!
//! - [`request`] — build and serialize DeepSeek requests from
//!   `vava-core` messages (the explicit DeepSeek serializer)
//! - [`response`] — the response shapes: chat completions, streaming
//!   chunks, usage, and API error bodies
//! - [`model`] — client configuration (model, thinking mode, base URL)
//! - `stream` — the SSE parser (next milestone)
//! - `client` — the HTTP client (a later milestone)
//!
//! It never executes tools, never touches the filesystem, and never prints.

pub mod model;
pub mod request;
pub mod response;

pub use model::{DEFAULT_BASE_URL, DEFAULT_MODEL, ModelConfig};
pub use request::{DeepSeekRequest, Role, Thinking, ThinkingMode, WireMessage, WireTool};
pub use response::{ApiErrorBody, ChatResponse, DeepSeekUsage, StreamChunk};
