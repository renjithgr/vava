//! Client configuration types.
//!
//! Configuration is deliberately plain data: model name, thinking mode, and
//! base URL. Nothing here hardcodes behavior based on a specific model name.

/// The default model vava talks to.
pub const DEFAULT_MODEL: &str = "deepseek-chat";

/// The official DeepSeek API base URL (no trailing slash).
pub const DEFAULT_BASE_URL: &str = "https://api.deepseek.com";

/// Configuration for the DeepSeek client.
///
/// `model` and `thinking` are independent on purpose: the user may combine
/// any model with any thinking setting, and vava must not assume that a
/// particular model name implies reasoning or not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelConfig {
    /// The model to query, e.g. `deepseek-chat` or `deepseek-reasoner`.
    pub model: String,
    /// Whether to request the model's thinking mode
    /// (DeepSeek's `thinking: {"type": "enabled"}` parameter).
    pub thinking: bool,
    /// Base URL of the API, without a trailing slash.
    pub base_url: String,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            model: DEFAULT_MODEL.to_string(),
            thinking: false,
            base_url: DEFAULT_BASE_URL.to_string(),
        }
    }
}

impl ModelConfig {
    /// A config with the given model and every other default.
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            ..Self::default()
        }
    }

    /// Enable or disable thinking mode.
    pub fn with_thinking(mut self, thinking: bool) -> Self {
        self.thinking = thinking;
        self
    }

    /// Override the API base URL.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_point_at_the_official_api() {
        let config = ModelConfig::default();
        assert_eq!(config.model, "deepseek-chat");
        assert!(!config.thinking);
        assert_eq!(config.base_url, "https://api.deepseek.com");
    }

    #[test]
    fn builder_overrides_individual_fields() {
        let config = ModelConfig::new("deepseek-reasoner")
            .with_thinking(true)
            .with_base_url("http://localhost:8080");
        assert_eq!(config.model, "deepseek-reasoner");
        assert!(config.thinking);
        assert_eq!(config.base_url, "http://localhost:8080");
        // Overriding one field must not disturb the others.
        let config = ModelConfig::default().with_thinking(true);
        assert_eq!(config.model, DEFAULT_MODEL);
        assert_eq!(config.base_url, DEFAULT_BASE_URL);
    }
}
