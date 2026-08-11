//! The vava desktop binary: initialize logging and run the Tauri app.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tracing_subscriber::EnvFilter;

fn main() {
    // The same tracing setup as the CLI: filter from RUST_LOG, never log
    // API keys or authorization headers (vava-deepseek never logs them).
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init()
        .ok();

    vava_desktop_lib::run();
}
