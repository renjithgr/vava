//! vava-deepseek — the DeepSeek API client for vava.
//!
//! This crate knows everything about the DeepSeek wire protocol and nothing
//! about repositories, tools, or terminals:
//!
//! - build and serialize DeepSeek requests from `vava-core` messages
//! - send them over HTTP
//! - consume the SSE stream and translate chunks into [`vava_core::ModelEvent`]s
//! - track token usage
//!
//! It never executes tools, never touches the filesystem, and never prints.
//!
//! Milestone 1: this crate exists as a workspace shell. The client, request
//! and response types, and the SSE parser arrive in milestones 2–3 and 6.
