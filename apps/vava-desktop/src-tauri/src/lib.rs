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
//! - [`model`]    — IPC data-transfer types (the Rust → React view layer)
//! - [`events`]   — the Rust → React event serialization boundary
//! - [`errors`]   — structured errors serialized to the frontend

pub mod commands;
pub mod errors;
pub mod events;
pub mod model;
pub mod state;

use tauri::Manager;

/// Run the Tauri application.
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // State construction can fail (no platform data directory);
            // abort startup with the message rather than panic.
            app.manage(state::DesktopState::new()?);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_version,
            commands::open_repository,
            commands::active_repository,
            commands::list_recent_repositories,
            commands::remove_recent_repository,
            commands::list_sessions,
            commands::select_session,
            commands::new_session,
            commands::send_prompt,
            commands::cancel_turn,
        ])
        .run(tauri::generate_context!())
        .expect("error while running vava desktop");
}
