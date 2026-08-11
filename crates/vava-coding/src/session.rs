//! CodingSession: turns the generic agent into a coding agent for one
//! repository.
//!
//! Responsibilities that belong here and nowhere else: the repository root,
//! project instructions (`AGENTS.md`), the coding system prompt, tool
//! registration, and session persistence (including resume).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use vava_core::{AgentError, AgentEvent, AgentHarness, Message, ModelClient, ToolRegistry};

use crate::context::{ContextError, ProjectContext};
use crate::persistence::{
    LoadedSession, PersistError, SessionId, SessionStore, SessionSummary, append_log,
};
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
    client: Arc<dyn ModelClient>,
    harness: AgentHarness,
    context: ProjectContext,
    session_store: SessionStore,
    session_id: SessionId,
    summary: SessionSummary,
    /// The log the message sink appends to, so switches can repoint it.
    log_path: PathBuf,
}

impl CodingSession {
    /// Open a session for the repository containing `start`, writing into
    /// the platform session store.
    pub fn open(client: Arc<dyn ModelClient>, start: &Path) -> Result<Self, SessionError> {
        Self::open_with_store(client, start, SessionStore::open()?)
    }

    /// Open a fresh session writing into an explicit store (used by tests).
    pub fn open_with_store(
        client: Arc<dyn ModelClient>,
        start: &Path,
        session_store: SessionStore,
    ) -> Result<Self, SessionError> {
        let context = ProjectContext::discover(start)?;
        let log = session_store.create(&context.root)?;
        let summary = session_store.summary(&log)?;
        let log_path = log.path().to_path_buf();
        let harness = Self::build_harness(&client, &context, &log_path, Vec::new());

        Ok(Self {
            root: context.root.clone(),
            client,
            context,
            session_store,
            session_id: summary.id.clone(),
            summary,
            log_path,
            harness,
        })
    }

    /// Open a session restored from a loaded transcript (session resume).
    pub fn resume_with_store(
        client: Arc<dyn ModelClient>,
        start: &Path,
        session_store: SessionStore,
        loaded: LoadedSession,
    ) -> Result<Self, SessionError> {
        let context = ProjectContext::discover(start)?;
        let log_path = loaded.log.path().to_path_buf();
        let summary = loaded.summary;
        let session_id = summary.id.clone();
        let harness = Self::build_harness(&client, &context, &log_path, loaded.messages);

        Ok(Self {
            root: context.root.clone(),
            client,
            context,
            session_store,
            session_id,
            summary,
            log_path,
            harness,
        })
    }

    /// Build the harness for a repository context: coding tools, the system
    /// prompt, a transcript (empty or restored), and a sink that appends
    /// every completed message to `log_path`.
    fn build_harness(
        client: &Arc<dyn ModelClient>,
        context: &ProjectContext,
        log_path: &Path,
        messages: Vec<Message>,
    ) -> AgentHarness {
        let mut registry = ToolRegistry::new();
        tools::register_coding_tools(&mut registry);
        let system = system_prompt(&context.root, context.agents_md.as_deref());

        let mut harness = AgentHarness::restored(
            client.clone(),
            registry,
            system,
            context.root.clone(),
            messages,
        );
        let log_path = log_path.to_path_buf();
        harness.set_message_sink(move |message| {
            if let Err(error) = append_log(&log_path, message) {
                tracing::warn!(%error, "could not persist session record");
            }
        });
        harness
    }

    /// The workspace boundary for tools.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The discovered repository context.
    pub fn context(&self) -> &ProjectContext {
        &self.context
    }

    /// The conversation transcript so far (restored messages included).
    pub fn messages(&self) -> &[Message] {
        self.harness.messages()
    }

    /// The id of this session's log file.
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// This session's metadata (id, timestamps, first prompt).
    pub fn summary(&self) -> &SessionSummary {
        &self.summary
    }

    /// The store this session's log lives in.
    pub fn session_store(&self) -> &SessionStore {
        &self.session_store
    }

    /// Start a brand-new session for the same repository, replacing the
    /// active transcript. The previous session is left untouched on disk.
    pub fn begin_new_session(&mut self) -> Result<SessionSummary, SessionError> {
        let log = self.session_store.create(&self.root)?;
        let summary = self.session_store.summary(&log)?;
        let log_path = log.path().to_path_buf();
        let harness = Self::build_harness(&self.client, &self.context, &log_path, Vec::new());
        self.harness = harness;
        self.session_id = summary.id.clone();
        self.summary = summary.clone();
        self.log_path = log_path;
        Ok(summary)
    }

    /// Switch this session to another (previously persisted) transcript of
    /// the same repository — the `/resume` flow. Future messages are
    /// appended to the resumed session's log, never to the abandoned one.
    pub fn resume_into(&mut self, loaded: LoadedSession) -> Result<(), SessionError> {
        // The picker is repository-scoped, so the loaded session belongs to
        // this repository; rediscover the context if that ever changes.
        if self.context.root != loaded.summary.repository_root {
            self.context = ProjectContext::discover(&loaded.summary.repository_root)?;
            self.root = self.context.root.clone();
        }
        let log_path = loaded.log.path().to_path_buf();
        let harness = Self::build_harness(&self.client, &self.context, &log_path, loaded.messages);
        self.harness = harness;
        self.session_id = loaded.summary.id.clone();
        self.summary = loaded.summary;
        self.log_path = log_path;
        Ok(())
    }

    /// Run one user prompt to completion, streaming [`AgentEvent`]s.
    ///
    /// `cancellation` scopes this single call; the caller owns it so each
    /// turn can get a fresh token.
    pub async fn prompt(
        &mut self,
        input: String,
        event_tx: mpsc::Sender<AgentEvent>,
        cancellation: CancellationToken,
    ) -> Result<(), AgentError> {
        self.harness.prompt(input, event_tx, cancellation).await
    }
}
