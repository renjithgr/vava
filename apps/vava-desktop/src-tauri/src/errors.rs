//! Structured errors returned by Tauri commands.
//!
//! Commands serialize these into JSON for React; the frontend renders the
//! concise `message` and can branch on the stable `kind`. Details stay in
//! the tracing logs — never dump backtraces into the normal UI.

use serde::Serialize;

/// A user-facing error from the desktop backend.
///
/// Each variant maps to a stage of the desktop workflow. `message` is what
/// React shows (concise, no backtraces).
#[derive(Debug, Clone, thiserror::Error)]
pub enum DesktopError {
    /// Opening or resolving a repository failed.
    #[error("unable to open repository: {0}")]
    Repository(String),
    /// Session listing, creation, or resume failed.
    #[error("session error: {0}")]
    Session(String),
    /// Desktop configuration (model, thinking, …) is invalid.
    #[error("configuration error: {0}")]
    Configuration(String),
    /// Storing or reading credentials (API key) failed.
    #[error("credential error: {0}")]
    Credential(String),
    /// The agent turn failed (model request, tool execution, …).
    #[error("agent error: {0}")]
    Agent(String),
    /// A filesystem operation failed.
    #[error("I/O error: {0}")]
    Io(String),
    /// An operation requires an open repository.
    #[error("no repository is open")]
    NoRepository,
    /// An operation requires an active session.
    #[error("no active session")]
    NoSession,
}

/// The serialized form sent to React.
#[derive(Debug, Serialize)]
pub struct DesktopErrorPayload {
    /// A stable machine-readable variant name the frontend can branch on.
    pub kind: String,
    /// A concise, user-facing message.
    pub message: String,
}

impl DesktopError {
    /// The stable variant name used by the frontend for branching.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Repository(_) => "repository",
            Self::Session(_) => "session",
            Self::Configuration(_) => "configuration",
            Self::Credential(_) => "credential",
            Self::Agent(_) => "agent",
            Self::Io(_) => "io",
            Self::NoRepository => "no_repository",
            Self::NoSession => "no_session",
        }
    }

    /// Build the IPC payload: variant name plus display message.
    pub fn into_payload(self) -> DesktopErrorPayload {
        DesktopErrorPayload {
            kind: self.kind().to_string(),
            message: self.to_string(),
        }
    }
}

/// `DesktopError` serializes as its payload (kind + message), so Tauri
/// commands can return it directly.
impl Serialize for DesktopError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.clone().into_payload().serialize(serializer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_has_a_stable_kind_and_concise_message() {
        let error = DesktopError::Agent("the DeepSeek request failed: rate limit exceeded".into());
        let payload = error.into_payload();
        assert_eq!(payload.kind, "agent");
        assert_eq!(
            payload.message,
            "agent error: the DeepSeek request failed: rate limit exceeded"
        );
    }

    #[test]
    fn serialization_matches_the_frontend_contract() {
        let error = DesktopError::NoRepository;
        let json = serde_json::to_string(&error).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"no_repository","message":"no repository is open"}"#
        );
    }

    #[test]
    fn every_variant_has_a_kind() {
        let errors = [
            DesktopError::Repository("x".into()),
            DesktopError::Session("x".into()),
            DesktopError::Configuration("x".into()),
            DesktopError::Credential("x".into()),
            DesktopError::Agent("x".into()),
            DesktopError::Io("x".into()),
            DesktopError::NoRepository,
            DesktopError::NoSession,
        ];
        for error in errors {
            assert!(!error.kind().is_empty());
        }
    }
}
