//! vava-coding — turns the generic agent into a coding agent.
//!
//! Responsibilities that belong here and nowhere else:
//!
//! - [`context`] — repository root discovery and project instructions
//!   (`AGENTS.md`)
//! - [`prompt`] — the coding system prompt
//! - [`session`] — [`CodingSession`], which binds one repository to a
//!   harness with the coding tools registered
//! - [`tools`] — the coding tools (`read`, `write`, `edit`, `bash`)
//! - [`persistence`] — append-only JSONL session logs
//!
//! The `AgentHarness` in `vava-core` stays generic: it knows how to run a
//! conversation, not what repository it is happening in.

pub mod context;
pub mod persistence;
pub mod prompt;
pub mod session;
pub mod tools;

pub use context::{ContextError, ProjectContext, find_repo_root, load_agents_md};
pub use persistence::{
    LoadedSession, PersistError, PrefixMatch, SessionHeader, SessionId, SessionLog, SessionStore,
    SessionSummary, resolve_prefix,
};
pub use prompt::system_prompt;
pub use session::{CodingSession, SessionError};
pub use tools::{PathError, resolve_within_root};
