//! Thin Tauri commands.
//!
//! Every command adapts one `vava-coding` / `vava-core` operation to IPC.
//! No agent logic lives here — this layer only moves data across the
//! boundary and reports failures through [`crate::errors::DesktopError`].
//! Commands grow with the milestones (open_repository in D2, sessions in
//! D3, send_prompt in D4, …).

use tauri::State;

use crate::state::DesktopState;

/// The desktop app's version, from this crate's manifest.
///
/// The first command: proves the React ↔ Rust IPC round trip works end to
/// end. It is pure, so it can be unit-tested without a Tauri runtime.
#[tauri::command]
pub fn get_version(_state: State<'_, DesktopState>) -> String {
    app_version()
}

/// The crate version, kept as a pure function for testing.
pub fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_version_is_a_semver_string() {
        let version = app_version();
        let parts: Vec<&str> = version.split('.').collect();
        assert!(parts.len() >= 2, "version should be at least major.minor");
        assert!(parts.iter().all(|part| !part.is_empty()));
    }
}
