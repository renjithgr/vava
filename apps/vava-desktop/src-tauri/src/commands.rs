//! Thin Tauri commands.
//!
//! Every command adapts one `vava-coding` / `vava-core` operation to IPC.
//! No agent logic lives here — this layer only moves data across the
//! boundary and reports failures through [`crate::errors::DesktopError`].
//! Commands grow with the milestones (sessions in D3, send_prompt in D4, …).

use tauri::State;

use crate::errors::DesktopError;
use crate::model::{RecentRepository, RepositoryInfo};
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

/// Open a repository (D2): resolve the workspace root, initialize a
/// session, list saved sessions, and record the repository in recents.
#[tauri::command]
pub async fn open_repository(
    path: String,
    state: State<'_, DesktopState>,
) -> Result<RepositoryInfo, DesktopError> {
    state.open_repository(&path).await
}

/// The currently open repository, if any.
#[tauri::command]
pub async fn active_repository(
    state: State<'_, DesktopState>,
) -> Result<Option<RepositoryInfo>, DesktopError> {
    state.active_repository().await
}

/// Recent repositories, newest first, for the launcher screen.
#[tauri::command]
pub fn list_recent_repositories(state: State<'_, DesktopState>) -> Vec<RecentRepository> {
    state.list_recent_repositories()
}

/// Remove a repository from recents (e.g. it no longer exists).
#[tauri::command]
pub fn remove_recent_repository(path: String, state: State<'_, DesktopState>) {
    state.remove_recent_repository(&path);
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
