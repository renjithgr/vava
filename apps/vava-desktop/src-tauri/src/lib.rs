//! vava-desktop — the Tauri backend for the vava desktop app.
//!
//! This crate is deliberately a *thin adapter*. All coding-agent behavior
//! comes from the reusable crates (`vava-coding`, `vava-core`,
//! `vava-deepseek`); this layer holds desktop state, exposes Tauri commands,
//! and translates agent events to IPC events. It must never re-implement
//! agent logic, and the reusable crates must never learn about Tauri.
//!
//! Layout:
//! - [`commands`] — Tauri command handlers (thin adapters over `vava-coding`)
//! - [`state`]    — desktop application state (repository, session, running turn)
//! - [`events`]   — the Rust → React event serialization boundary
//! - [`errors`]   — structured errors serialized to the frontend

pub mod commands;
pub mod errors;
pub mod events;
pub mod state;

/// Run the Tauri application.
pub fn run() {
    tauri::Builder::default()
        .manage(state::DesktopState::new())
        .invoke_handler(tauri::generate_handler![commands::get_version])
        .run(tauri::generate_context!())
        .expect("error while running vava desktop");
}
