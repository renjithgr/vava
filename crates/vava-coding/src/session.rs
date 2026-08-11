//! CodingSession: turns the generic agent into a coding agent for one
//! repository.
//!
//! Responsibilities that belong here and nowhere else: the repository root,
//! project instructions (`AGENTS.md`), the coding system prompt, tool
//! registration, and session persistence.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use vava_core::{AgentError, AgentEvent, AgentHarness, Message, ModelClient, ToolRegistry};

use crate::context::{ContextError, ProjectContext};
use crate::persistence::{PersistError, SessionStore, append_log};
use crate::prompt::system_prompt;
use crate::tools;

/// Errors from opening or running a coding session.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error(transparent)]
    Context(#[from] ContextError),
    #[error(transparent)]
    Persist(#[from] PersistError),
}

/// A coding agent bound to one repository.
pub struct CodingSession {
    root: PathBuf,
    harness: AgentHarness,
    context: ProjectContext,
    session_store: SessionStore,
    session_id: String,
}

impl CodingSession {
    /// Open a session for the repository containing `start`, writing into
    /// the platform session store.
    pub fn open(client: Arc<dyn ModelClient>, start: &Path) -> Result<Self, SessionError> {
        Self::open_with_store(client, start, SessionStore::open()?)
    }

    /// Open a session writing into an explicit store (used by tests and,
    /// later, session resume).
    pub fn open_with_store(
        client: Arc<dyn ModelClient>,
        start: &Path,
        session_store: SessionStore,
    ) -> Result<Self, SessionError> {
        let context = ProjectContext::discover(start)?;
        let mut registry = ToolRegistry::new();
        tools::register_coding_tools(&mut registry);
        let system = system_prompt(&context.root, context.agents_md.as_deref());

        let log = session_store.create(&context.root)?;
        let session_id = log.id().to_string();
        let log_path = log.path().to_path_buf();

        let mut harness = AgentHarness::new(client, registry, system, context.root.clone());
        // Every completed transcript message is appended to the log as it
        // happens (a tiny synchronous write + flush).
        harness.set_message_sink(move |message| {
            if let Err(error) = append_log(&log_path, message) {
                tracing::warn!(%error, "could not persist session record");
            }
        });

        Ok(Self {
            root: context.root.clone(),
            harness,
            context,
            session_store,
            session_id,
        })
    }

    /// The workspace boundary for tools.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The discovered repository context.
    pub fn context(&self) -> &ProjectContext {
        &self.context
    }

    /// The conversation transcript so far.
    pub fn messages(&self) -> &[Message] {
        self.harness.messages()
    }

    /// The id of this session's log file.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// The store this session's log lives in.
    pub fn session_store(&self) -> &SessionStore {
        &self.session_store
    }

    /// A handle to the session's cancellation token.
    pub fn cancellation_token(&self) -> CancellationToken {
        self.harness.cancellation_token()
    }

    /// Cancel the current operation.
    pub fn cancel(&self) {
        self.harness.cancel();
    }

    /// Run one user prompt to completion, streaming [`AgentEvent`]s.
    pub async fn prompt(
        &mut self,
        input: String,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<(), AgentError> {
        self.harness.prompt(input, event_tx).await
    }
}
