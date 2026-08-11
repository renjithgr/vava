//! vava-coding — turns the generic agent into a coding agent.
//!
//! Responsibilities that belong here and nowhere else:
//!
//! - locating the repository root
//! - loading project instructions (`AGENTS.md`)
//! - building the coding system prompt
//! - registering the coding tools (`read`, `write`, `edit`, `bash`)
//! - persisting and restoring sessions (append-only JSONL)
//!
//! The `AgentHarness` in `vava-core` stays generic: it knows how to run a
//! conversation, not what repository it is happening in.
//!
//! Milestone 1: this crate exists as a workspace shell. Its modules arrive
//! in milestones 7–11.
