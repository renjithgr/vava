//! The Rust → React event serialization boundary.
//!
//! `vava-core`'s [`AgentEvent`]s are the frontend-neutral contract between
//! the harness and any UI. This module translates them into the JSON events
//! streamed to React through Tauri channels (milestone D4). Nothing here
//! may leak into `vava-core`; this is the single, explicit boundary.

// The `DesktopAgentEvent` types land with D4 (prompt streaming). The module
// exists from D1 so the serialization boundary has one home and the crate
// layout is stable.
