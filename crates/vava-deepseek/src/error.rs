//! Errors raised by the DeepSeek layer.
//!
//! The HTTP/API variants arrive in a later milestone, when the reqwest-based
//! client is added; this milestone only needs framing and protocol errors.

use thiserror::Error;

/// Errors from talking to the DeepSeek API.
#[derive(Debug, Error)]
pub enum DeepSeekError {
    /// The SSE framing was malformed (e.g. a data field that is not valid
    /// UTF-8).
    #[error("SSE parse error: {0}")]
    Sse(String),

    /// A payload that does not match the expected protocol shape.
    #[error("DeepSeek protocol error: {0}")]
    Protocol(String),

    /// The operation was cancelled.
    #[error("operation cancelled")]
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_display_helpfully() {
        assert_eq!(
            DeepSeekError::Sse("bad utf-8".into()).to_string(),
            "SSE parse error: bad utf-8"
        );
        assert_eq!(
            DeepSeekError::Protocol("missing id".into()).to_string(),
            "DeepSeek protocol error: missing id"
        );
        assert_eq!(DeepSeekError::Cancelled.to_string(), "operation cancelled");
    }
}
