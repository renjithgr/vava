//! CodingSession: turns the generic agent into a coding agent for one
//! repository.
//!
//! Responsibilities that belong here and nowhere else: the repository root,
//! project instructions (`AGENTS.md`), the coding system prompt, tool
//! registration, and — in the next milestone — session persistence.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use vava_core::{AgentError, AgentEvent, AgentHarness, Message, ModelClient, ToolRegistry};

use crate::context::{ContextError, ProjectContext};
use crate::prompt::system_prompt;
use crate::tools;

/// A coding agent bound to one repository.
pub struct CodingSession {
    root: PathBuf,
    harness: AgentHarness,
    context: ProjectContext,
}

impl CodingSession {
    /// Open a session for the repository containing `start`.
    ///
    /// Discovers the repository root, loads `AGENTS.md`, builds the system
    /// prompt, and registers the coding tools. The harness stays generic;
    /// all repository knowledge lives here.
    pub fn open(client: Arc<dyn ModelClient>, start: &Path) -> Result<Self, ContextError> {
        let context = ProjectContext::discover(start)?;
        let mut registry = ToolRegistry::new();
        tools::register_coding_tools(&mut registry);
        let system = system_prompt(&context.root, context.agents_md.as_deref());
        let harness = AgentHarness::new(client, registry, system, context.root.clone());
        Ok(Self {
            root: context.root.clone(),
            harness,
            context,
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
