//! Errors raised by the DeepSeek layer.

use thiserror::Error;

/// Errors from talking to the DeepSeek API.
#[derive(Debug, Error)]
pub enum DeepSeekError {
    /// The HTTP request itself failed (network, DNS, timeout, ...).
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    /// DeepSeek returned a non-success status with an error body.
    #[error("DeepSeek API error (HTTP {status}): {message}")]
    Api {
        /// The HTTP status code.
        status: u16,
        /// The message from the API error body.
        message: String,
    },

    /// The SSE framing was malformed (e.g. a data field that is not valid
    /// UTF-8).
    #[error("SSE parse error: {0}")]
    Sse(String),

    /// A payload that does not match the expected protocol shape.
    #[error("DeepSeek protocol error: {0}")]
    Protocol(String),
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
        assert_eq!(
            DeepSeekError::Api {
                status: 401,
                message: "Invalid API key".into()
            }
            .to_string(),
            "DeepSeek API error (HTTP 401): Invalid API key"
        );
    }
}
