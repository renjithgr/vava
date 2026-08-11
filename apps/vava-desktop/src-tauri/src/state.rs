//! Desktop application state.
//!
//! D1 keeps the state minimal: a marker with no fields. The repository and
//! session state (Phase 3) lands here in the D2–D4 milestones:
//!
//! ```text
//! DesktopState
//!   └── active repository
//!         ├── CodingSession (moved into the turn task while running)
//!         ├── running turn + cancellation token
//!         └── session store
//! ```
//!
//! Ownership is structured so only one turn per repository executes at a
//! time, without a single enormous `Arc<Mutex<AppState>>`.

/// Application-wide state managed by Tauri and handed to every command.
///
/// D1: a marker only — the first real fields arrive with D2.
#[derive(Debug, Default)]
pub struct DesktopState;

impl DesktopState {
    /// Create the initial application state.
    pub fn new() -> Self {
        Self
    }
}
