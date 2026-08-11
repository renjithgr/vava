//! Desktop application state.
//!
//! D2 adds the active repository (repository context, optional
//! [`CodingSession`]) and the recent-repositories list persisted to a small
//! JSON file. The running-turn/cancellation machinery arrives with D4;
//! ownership stays structured so only one turn per repository executes at a
//! time, without one enormous `Arc<Mutex<AppState>>`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tokio::sync::{Mutex as AsyncMutex, mpsc, watch};
use tokio_util::sync::CancellationToken;

use vava_coding::{CodingSession, ProjectContext, SessionId, SessionStore, SessionSummary};
use vava_core::{AgentEvent, Message, ModelClient};
use vava_deepseek::{DeepSeekClient, ModelConfig};

use crate::errors::DesktopError;
use crate::events::DesktopAgentEvent;
use crate::model::{DesktopMessage, RecentRepository, RepositoryInfo, SessionInfo, SessionView};

/// How many recent repositories to keep.
const MAX_RECENTS: usize = 10;

/// Application-wide state managed by Tauri and handed to every command.
pub struct DesktopState {
    /// The currently open repository, if any. Shared with turn tasks
    /// via `Arc`, so a task can write the session back after finishing.
    active: Arc<AsyncMutex<Option<ActiveRepository>>>,
    /// The persisted recent-repository list.
    recents: std::sync::Mutex<Recents>,
    /// The platform session store (shared by all commands).
    store: SessionStore,
    /// The model client, when an API key is configured. Built once at
    /// startup from the environment (like the CLI); D9 adds a settings
    /// screen that rebuilds it from the keychain.
    client: Option<Arc<dyn ModelClient>>,
}

/// The open repository and its session machinery.
pub struct ActiveRepository {
    root: PathBuf,
    context: ProjectContext,
    /// The active `CodingSession`, while no turn is running. Moved into the
    /// turn task during a turn and written back when it finishes.
    session: Option<CodingSession>,
    /// The in-flight turn, if any. Only one turn executes at a time.
    running_turn: Option<RunningTurn>,
    /// Monotonic id for naming turns (used to detect stale write-backs).
    next_turn_id: u64,
}

/// An in-flight agent turn.
pub struct RunningTurn {
    id: u64,
    cancellation: CancellationToken,
    /// The turn task sends on this channel after it has written the session
    /// back into the repository state, so waiters know the turn is over.
    completed: watch::Sender<()>,
}

/// Everything `send_prompt` extracted from the state to hand to the turn
/// task: the session (moved out), the cancellation token, and the
/// completion signal.
pub struct TurnStart {
    pub session: CodingSession,
    pub id: u64,
    pub token: CancellationToken,
    pub completed: watch::Sender<()>,
}

impl Default for DesktopState {
    fn default() -> Self {
        Self::new().expect("desktop state requires a session data directory")
    }
}

impl DesktopState {
    /// Create the initial application state.
    pub fn new() -> Result<Self, DesktopError> {
        Ok(Self {
            active: Arc::new(AsyncMutex::new(None)),
            recents: std::sync::Mutex::new(Recents::open()),
            store: SessionStore::open()
                .map_err(|error| DesktopError::Session(error.to_string()))?,
            client: model_client_from_env(),
        })
    }

    /// A state with injected store/recents/client (used by tests).
    #[cfg(test)]
    pub fn for_test(
        store: SessionStore,
        recents: Recents,
        client: Option<Arc<dyn ModelClient>>,
    ) -> Self {
        Self {
            active: Arc::new(AsyncMutex::new(None)),
            recents: std::sync::Mutex::new(recents),
            store,
            client,
        }
    }

    /// The shared store handle (for tests).
    #[cfg(test)]
    pub fn store(&self) -> &SessionStore {
        &self.store
    }

    /// Open the repository containing `path`.
    ///
    /// Resolves the workspace root, initializes a [`CodingSession`] when a
    /// model client is available (currently from `DEEPSEEK_API_KEY`, like
    /// the CLI), lists saved sessions, and records the repository in
    /// recents.
    pub async fn open_repository(&self, path: &str) -> Result<RepositoryInfo, DesktopError> {
        let opened = {
            let mut recents = self.recents.lock().unwrap();
            open_repository_inner(
                Path::new(path),
                &self.store,
                &mut recents,
                self.client.clone(),
            )?
        };
        *self.active.lock().await = Some(ActiveRepository {
            root: opened.context.root.clone(),
            context: opened.context,
            session: opened.session,
            running_turn: None,
            next_turn_id: 0,
        });
        Ok(opened.info)
    }

    /// The currently open repository, if any (sessions re-listed live).
    pub async fn active_repository(&self) -> Result<Option<RepositoryInfo>, DesktopError> {
        let active = self.active.lock().await;
        let Some(active) = active.as_ref() else {
            return Ok(None);
        };
        let sessions = self
            .store
            .list_for_repository(&active.root)
            .map_err(|error| DesktopError::Session(error.to_string()))?;
        let active_session_id = active
            .session
            .as_ref()
            .map(|session| session.session_id().as_str());
        Ok(Some(build_repository_info(
            &active.context,
            &sessions,
            active_session_id,
        )))
    }

    /// Recent repositories, newest first, for the launcher screen.
    pub fn list_recent_repositories(&self) -> Vec<RecentRepository> {
        let recents = self.recents.lock().unwrap();
        recents.list().iter().map(recent_repository).collect()
    }

    /// Remove a repository from recents (e.g. it no longer exists).
    pub fn remove_recent_repository(&self, path: &str) {
        let mut recents = self.recents.lock().unwrap();
        recents.remove(Path::new(path));
    }

    /// The active repository's sessions, newest first.
    pub async fn list_sessions(&self) -> Result<Vec<SessionInfo>, DesktopError> {
        let active = self.active.lock().await;
        let repo = active.as_ref().ok_or(DesktopError::NoRepository)?;
        let sessions = self
            .store
            .list_for_repository(&repo.root)
            .map_err(|error| DesktopError::Session(error.to_string()))?;
        Ok(sessions.iter().map(session_info).collect())
    }

    /// Switch the active session to a previously persisted transcript (D3).
    ///
    /// Future messages are appended to the selected session's log; the
    /// abandoned session is left untouched on disk. Any running turn is
    /// cancelled first (Phase 7).
    pub async fn select_session(&self, id: &str) -> Result<SessionView, DesktopError> {
        self.stop_running_turn().await?;
        let mut active = self.active.lock().await;
        let repo = active.as_mut().ok_or(DesktopError::NoRepository)?;
        let session = repo.session.as_mut().ok_or_else(no_api_key_error)?;
        select_session_inner(session, &self.store, id)
    }

    /// Start a brand-new session for the active repository (D3).
    ///
    /// Equivalent to the terminal `/new`: a fresh transcript, the previous
    /// session left untouched. Repository, project context, and file index
    /// are preserved. Any running turn is cancelled first.
    pub async fn new_session(&self) -> Result<SessionView, DesktopError> {
        self.stop_running_turn().await?;
        let mut active = self.active.lock().await;
        let repo = active.as_mut().ok_or(DesktopError::NoRepository)?;
        let session = repo.session.as_mut().ok_or_else(no_api_key_error)?;
        new_session_inner(session)
    }

    /// Run one user prompt (D4), streaming [`DesktopAgentEvent`]s to the
    /// Tauri channel.
    ///
    /// Starts the turn immediately and returns; the frontend receives the
    /// stream through the channel. Only one turn runs at a time: any
    /// existing turn is cancelled and awaited first. The session is taken
    /// out of the state for the duration of the turn and written back by
    /// the turn task when it finishes.
    pub async fn send_prompt(
        &self,
        session_id: &str,
        input: &str,
        channel: Channel<DesktopAgentEvent>,
    ) -> Result<(), DesktopError> {
        if input.trim().is_empty() {
            return Err(DesktopError::Configuration(
                "prompt must not be empty".into(),
            ));
        }
        self.stop_running_turn().await?;
        let start = self.begin_turn(session_id).await?;

        // The event pipeline: harness events → mpsc → translation → Tauri
        // channel. The turn task owns the session and returns it to the
        // state when done.
        let (event_tx, event_rx) = mpsc::channel(64);
        tokio::spawn(forward_events(event_rx, channel));
        let inner = self.active.clone();
        tokio::spawn(run_turn(
            inner,
            start.session,
            input.to_string(),
            start.token,
            start.id,
            start.completed,
            event_tx,
        ));
        Ok(())
    }

    /// Take the active session out of the state and register a running
    /// turn for it. Rejects ids that are not the active session.
    async fn begin_turn(&self, session_id: &str) -> Result<TurnStart, DesktopError> {
        let mut guard = self.active.lock().await;
        let repo = guard.as_mut().ok_or(DesktopError::NoRepository)?;
        let session = repo.session.take().ok_or_else(no_api_key_error)?;
        if session.session_id().as_str() != session_id {
            repo.session = Some(session);
            return Err(DesktopError::Session(format!(
                "session `{session_id}` is not the active session"
            )));
        }
        let id = {
            repo.next_turn_id += 1;
            repo.next_turn_id
        };
        let token = CancellationToken::new();
        let (completed, _) = watch::channel(());
        repo.running_turn = Some(RunningTurn {
            id,
            cancellation: token.clone(),
            completed: completed.clone(),
        });
        Ok(TurnStart {
            session,
            id,
            token,
            completed,
        })
    }

    /// Cancel any running turn and wait until the session has been written
    /// back into the state. Used before switching sessions or starting a
    /// new turn, so the state never holds two turns or loses a session.
    pub async fn stop_running_turn(&self) -> Result<(), DesktopError> {
        let subscription = {
            let mut guard = self.active.lock().await;
            let repo = guard.as_mut().ok_or(DesktopError::NoRepository)?;
            match repo.running_turn.as_ref() {
                Some(turn) => {
                    tracing::debug!(turn_id = turn.id, "cancelling running turn");
                    turn.cancellation.cancel();
                    Some(turn.completed.subscribe())
                }
                None => None,
            }
        };
        let Some(mut completed) = subscription else {
            return Ok(());
        };
        loop {
            {
                let guard = self.active.lock().await;
                let clear = guard
                    .as_ref()
                    .map(|repo| repo.running_turn.is_none())
                    .unwrap_or(true);
                if clear {
                    return Ok(());
                }
            }
            if completed.has_changed().unwrap_or(true) {
                continue;
            }
            if completed.changed().await.is_err() {
                // The turn task is gone; nothing more to wait for.
                return Ok(());
            }
        }
    }

    /// Cancel the running turn without waiting (the frontend Stop button).
    /// The turn ends asynchronously; the session is written back by the
    /// turn task and the harness streams an error event.
    pub async fn cancel_turn(&self) -> Result<(), DesktopError> {
        let guard = self.active.lock().await;
        let repo = guard.as_ref().ok_or(DesktopError::NoRepository)?;
        if let Some(turn) = repo.running_turn.as_ref() {
            tracing::debug!(turn_id = turn.id, "cancelling running turn (stop)");
            turn.cancellation.cancel();
        }
        Ok(())
    }

    /// Whether the active session is currently in the state (tests).
    #[cfg(test)]
    pub async fn active_session_present(&self) -> bool {
        let guard = self.active.lock().await;
        guard
            .as_ref()
            .map(|repo| repo.session.is_some())
            .unwrap_or(false)
    }

    /// A clone of the active-state handle, for spawning turn tasks (tests).
    #[cfg(test)]
    pub fn active_handle(&self) -> Arc<AsyncMutex<Option<ActiveRepository>>> {
        Arc::clone(&self.active)
    }
}

/// The result of opening a repository: the info for React plus the session
/// machinery to keep in state.
pub struct OpenedRepository {
    pub info: RepositoryInfo,
    pub context: ProjectContext,
    pub session: Option<CodingSession>,
}

/// Resolve a repository root, initialize a session when a client is
/// available, list saved sessions, and record the repository in recents.
///
/// Pure application logic, kept free of Tauri so it is unit-testable.
pub fn open_repository_inner(
    path: &Path,
    store: &SessionStore,
    recents: &mut Recents,
    client: Option<Arc<dyn ModelClient>>,
) -> Result<OpenedRepository, DesktopError> {
    let context = ProjectContext::discover(path)
        .map_err(|error| DesktopError::Repository(error.to_string()))?;

    // A fresh session is created first so the repository opens ready to
    // prompt (the CLI equivalent of starting a session); the session list
    // below then includes it, newest first.
    let session = match client {
        Some(client) => {
            let session = CodingSession::open_with_store(client, &context.root, store.clone())
                .map_err(|error| DesktopError::Repository(error.to_string()))?;
            Some(session)
        }
        None => None,
    };

    let sessions = store
        .list_for_repository(&context.root)
        .map_err(|error| DesktopError::Session(error.to_string()))?;

    recents.touch(&context.root);

    let info = build_repository_info(
        &context,
        &sessions,
        session
            .as_ref()
            .map(|session| session.session_id().as_str()),
    );
    Ok(OpenedRepository {
        info,
        context,
        session,
    })
}

/// Load a session by id and switch the active `CodingSession` to it,
/// returning the restored transcript view.
///
/// Pure application logic (free of Tauri) so it is unit-testable; the
/// caller owns the active session and any in-flight turn cancellation.
pub fn select_session_inner(
    session: &mut CodingSession,
    store: &SessionStore,
    id: &str,
) -> Result<SessionView, DesktopError> {
    let loaded = store
        .load(&SessionId::new(id))
        .map_err(|error| DesktopError::Session(error.to_string()))?;
    let summary = loaded.summary.clone();
    session
        .resume_into(loaded)
        .map_err(|error| DesktopError::Session(error.to_string()))?;
    Ok(build_session_view(&summary, session.messages()))
}

/// Start a brand-new session on the active `CodingSession`, returning the
/// (empty) transcript view.
pub fn new_session_inner(session: &mut CodingSession) -> Result<SessionView, DesktopError> {
    let summary = session
        .begin_new_session()
        .map_err(|error| DesktopError::Session(error.to_string()))?;
    Ok(build_session_view(&summary, session.messages()))
}

/// The IPC view of a loaded session: summary plus converted transcript.
fn build_session_view(summary: &SessionSummary, messages: &[Message]) -> SessionView {
    SessionView {
        session: session_info(summary),
        messages: messages.iter().map(DesktopMessage::from).collect(),
    }
}

/// The user-facing error when no model client is available.
fn no_api_key_error() -> DesktopError {
    DesktopError::Configuration("DeepSeek API key is not configured.".into())
}

/// Run one agent turn to completion, then return the session to the state.
///
/// Owns the session for the duration of the turn; the harness streams
/// [`AgentEvent`]s through `event_tx`. On completion (success, error, or
/// cancellation) the session is written back into `inner` and the
/// completion channel is signaled, so waiters (session switches, the next
/// turn) know the turn is over. The write-back is skipped only if a newer
/// turn has already claimed the slot (defensive — callers serialize turns
/// through `stop_running_turn`, so this never happens in practice).
pub async fn run_turn(
    inner: Arc<AsyncMutex<Option<ActiveRepository>>>,
    mut session: CodingSession,
    input: String,
    token: CancellationToken,
    turn_id: u64,
    completed_tx: watch::Sender<()>,
    event_tx: mpsc::Sender<AgentEvent>,
) {
    let result = session.prompt(input, event_tx, token).await;
    if let Err(error) = &result {
        tracing::debug!(%error, "turn finished with error");
    }
    {
        let mut guard = inner.lock().await;
        if let Some(repo) = guard.as_mut()
            && repo.running_turn.as_ref().map(|turn| turn.id) == Some(turn_id)
        {
            repo.session = Some(session);
            repo.running_turn = None;
        }
    }
    let _ = completed_tx.send(());
}

/// Translate harness events to desktop IPC events and send them to the
/// Tauri channel until the stream ends or the frontend disconnects.
async fn forward_events(mut rx: mpsc::Receiver<AgentEvent>, channel: Channel<DesktopAgentEvent>) {
    while let Some(event) = rx.recv().await {
        if channel.send(DesktopAgentEvent::from(&event)).is_err() {
            tracing::debug!("frontend event channel closed; stopping forwarding");
            break;
        }
    }
}

/// Build the IPC view of a repository and its sessions.
fn build_repository_info(
    context: &ProjectContext,
    sessions: &[SessionSummary],
    active_session_id: Option<&str>,
) -> RepositoryInfo {
    RepositoryInfo {
        id: vava_coding::persistence::repository_key(&context.root),
        name: repository_name(&context.root),
        root: context.root.display().to_string(),
        active_session_id: active_session_id.map(str::to_string),
        sessions: sessions.iter().map(session_info).collect(),
    }
}

/// The IPC view of one session summary.
fn session_info(summary: &SessionSummary) -> SessionInfo {
    SessionInfo {
        id: summary.id.as_str().to_string(),
        created_at: summary.created_at.to_rfc3339(),
        updated_at: summary.updated_at.to_rfc3339(),
        first_user_message: summary.first_user_message.clone(),
    }
}

/// The IPC view of one recent-repository entry.
fn recent_repository(entry: &RecentEntry) -> RecentRepository {
    RecentRepository {
        path: entry.path.clone(),
        name: repository_name(Path::new(&entry.path)),
        last_opened_at: entry.last_opened_at.clone(),
        exists: Path::new(&entry.path).exists(),
    }
}

/// The repository's display name: its directory name, or the path itself
/// for a root without one.
fn repository_name(root: &Path) -> String {
    root.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string())
}

/// Build a DeepSeek model client from the environment, matching the CLI.
///
/// D9 replaces this with keychain-backed desktop settings. The key itself
/// is never logged.
fn model_client_from_env() -> Option<Arc<dyn ModelClient>> {
    match std::env::var("DEEPSEEK_API_KEY") {
        Ok(key) if !key.trim().is_empty() => {
            let config = ModelConfig::new(vava_deepseek::DEFAULT_MODEL);
            Some(Arc::new(DeepSeekClient::new(
                secrecy::SecretString::from(key),
                config,
            )))
        }
        Ok(_) | Err(_) => {
            tracing::info!(
                "DEEPSEEK_API_KEY not set; sessions can be browsed but prompts are unavailable"
            );
            None
        }
    }
}

/// The persisted recent-repository list.
///
/// A small JSON file (no database): `<data_dir>/vava/desktop/recents.json`.
/// Persistence is best-effort — failures are logged, never fatal.
pub struct Recents {
    path: Option<PathBuf>,
    entries: Vec<RecentEntry>,
}

/// One recent repository, as stored on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentEntry {
    pub path: String,
    /// RFC 3339 timestamp of the last open.
    #[serde(default)]
    pub last_opened_at: String,
}

#[derive(Serialize, Deserialize)]
struct RecentsFile {
    #[serde(default)]
    entries: Vec<RecentEntry>,
}

impl Recents {
    /// The default location: `<data_dir>/vava/desktop/recents.json`.
    pub fn open() -> Self {
        let path = dirs::data_local_dir()
            .map(|base| base.join("vava").join("desktop").join("recents.json"));
        let entries = match &path {
            Some(path) => Self::load(path),
            None => Vec::new(),
        };
        Self { path, entries }
    }

    /// A store rooted at an explicit file (used by tests).
    pub fn open_at(path: PathBuf) -> Self {
        let entries = Self::load(&path);
        Self {
            path: Some(path),
            entries,
        }
    }

    /// An in-memory store that never touches disk (used by tests).
    pub fn in_memory() -> Self {
        Self {
            path: None,
            entries: Vec::new(),
        }
    }

    fn load(path: &Path) -> Vec<RecentEntry> {
        match std::fs::read_to_string(path) {
            Ok(content) => match serde_json::from_str::<RecentsFile>(&content) {
                Ok(file) => file.entries,
                Err(error) => {
                    tracing::warn!(path = %path.display(), %error, "could not parse recents file; starting empty");
                    Vec::new()
                }
            },
            Err(_) => Vec::new(),
        }
    }

    fn persist(&self) {
        let Some(path) = &self.path else {
            return;
        };
        let file = RecentsFile {
            entries: self.entries.clone(),
        };
        let json = match serde_json::to_string_pretty(&file) {
            Ok(json) => json,
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "could not serialize recents");
                return;
            }
        };
        let result = (|| -> std::io::Result<()> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, json)
        })();
        if let Err(error) = result {
            tracing::warn!(path = %path.display(), %error, "could not persist recents");
        }
    }

    /// All entries, newest first.
    pub fn list(&self) -> &[RecentEntry] {
        &self.entries
    }

    /// Record that `root` was just opened: dedupe by path, move to the
    /// front, cap the list, persist.
    pub fn touch(&mut self, root: &Path) {
        let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        self.entries.retain(|entry| {
            std::fs::canonicalize(&entry.path).unwrap_or_else(|_| PathBuf::from(&entry.path))
                != canonical
        });
        self.entries.insert(
            0,
            RecentEntry {
                path: canonical.display().to_string(),
                last_opened_at: chrono::Utc::now().to_rfc3339(),
            },
        );
        self.entries.truncate(MAX_RECENTS);
        self.persist();
    }

    /// Remove a repository from the list.
    pub fn remove(&mut self, root: &Path) {
        self.entries.retain(|entry| Path::new(&entry.path) != root);
        self.persist();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use futures::stream::BoxStream;
    use vava_core::{Message, ModelEvent, ToolDefinition, UserMessage};

    /// A unique temporary directory that cleans itself up on drop.
    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let n = NEXT.fetch_add(1, Ordering::SeqCst);
            let dir =
                std::env::temp_dir().join(format!("vava-desktop-test-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let dir = std::fs::canonicalize(&dir).unwrap();
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn child(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A model client that streams nothing (enough to open a session).
    struct FakeClient;

    #[async_trait::async_trait]
    impl ModelClient for FakeClient {
        async fn stream(
            &self,
            _messages: &[Message],
            _system: &str,
            _tools: &[ToolDefinition],
        ) -> Result<
            BoxStream<'static, Result<ModelEvent, vava_core::BoxedError>>,
            vava_core::BoxedError,
        > {
            Ok(Box::pin(futures::stream::empty()))
        }
    }

    /// A model client that plays back scripted events, then ends the
    /// stream.
    struct ScriptedClient {
        events: Vec<ModelEvent>,
    }

    impl ScriptedClient {
        fn new(events: Vec<ModelEvent>) -> Self {
            Self { events }
        }
    }

    #[async_trait::async_trait]
    impl ModelClient for ScriptedClient {
        async fn stream(
            &self,
            _messages: &[Message],
            _system: &str,
            _tools: &[ToolDefinition],
        ) -> Result<
            BoxStream<'static, Result<ModelEvent, vava_core::BoxedError>>,
            vava_core::BoxedError,
        > {
            let events = self.events.clone();
            Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
        }
    }

    /// A model client whose stream never produces anything, so the turn
    /// only ends through cancellation.
    struct PendingClient;

    #[async_trait::async_trait]
    impl ModelClient for PendingClient {
        async fn stream(
            &self,
            _messages: &[Message],
            _system: &str,
            _tools: &[ToolDefinition],
        ) -> Result<
            BoxStream<'static, Result<ModelEvent, vava_core::BoxedError>>,
            vava_core::BoxedError,
        > {
            Ok(Box::pin(futures::stream::pending()))
        }
    }

    fn user(content: &str) -> Message {
        Message::User(UserMessage {
            content: content.into(),
        })
    }

    /// A session store rooted in a directory that lives for the test.
    fn test_store() -> (SessionStore, TestDir) {
        let dir = TestDir::new();
        (
            SessionStore::open_at(dir.path().to_path_buf()).unwrap(),
            dir,
        )
    }

    #[test]
    fn open_repository_resolves_root_and_lists_sessions() {
        let repo = TestDir::new();
        std::fs::create_dir_all(repo.child(".git")).unwrap();
        let (store, _dir) = test_store();
        // A previously saved session for this repository.
        let log = store.create(repo.path()).unwrap();
        log.append(&user("fix the tests")).unwrap();

        let mut recents = Recents::in_memory();
        let opened = open_repository_inner(
            repo.path(),
            &store,
            &mut recents,
            Some(Arc::new(FakeClient)),
        )
        .unwrap();

        assert_eq!(
            opened.info.name,
            repo.path().file_name().unwrap().to_str().unwrap()
        );
        assert_eq!(opened.info.root, repo.path().display().to_string());
        // The fresh session created at open plus the saved one.
        assert_eq!(opened.info.sessions.len(), 2);
        assert!(opened.info.active_session_id.is_some());
        assert!(opened.session.is_some());
        // Recents were touched.
        assert_eq!(recents.list().len(), 1);
    }

    #[test]
    fn open_repository_from_a_nested_directory_uses_the_repo_root() {
        let repo = TestDir::new();
        std::fs::create_dir_all(repo.child(".git")).unwrap();
        std::fs::create_dir_all(repo.child("src/deep")).unwrap();
        let (store, _dir) = test_store();
        let mut recents = Recents::in_memory();

        let opened =
            open_repository_inner(&repo.child("src/deep"), &store, &mut recents, None).unwrap();

        assert_eq!(opened.info.root, repo.path().display().to_string());
        assert_eq!(
            opened.info.name,
            repo.path().file_name().unwrap().to_str().unwrap()
        );
    }

    #[test]
    fn open_repository_without_client_lists_sessions_only() {
        let repo = TestDir::new();
        std::fs::create_dir_all(repo.child(".git")).unwrap();
        let (store, _dir) = test_store();
        let log = store.create(repo.path()).unwrap();
        log.append(&user("old prompt")).unwrap();

        let mut recents = Recents::in_memory();
        let opened = open_repository_inner(repo.path(), &store, &mut recents, None).unwrap();

        assert_eq!(opened.info.sessions.len(), 1);
        assert_eq!(
            opened.info.sessions[0].first_user_message.as_deref(),
            Some("old prompt")
        );
        assert!(opened.info.active_session_id.is_none());
        assert!(opened.session.is_none());
    }

    #[test]
    fn open_repository_rejects_a_missing_path() {
        let (store, _dir) = test_store();
        let mut recents = Recents::in_memory();
        let err = match open_repository_inner(
            Path::new("/definitely/not/here"),
            &store,
            &mut recents,
            None,
        ) {
            Ok(_) => panic!("expected an error"),
            Err(err) => err,
        };
        assert!(matches!(err, DesktopError::Repository(_)));
    }

    #[test]
    fn recents_dedupe_and_cap() {
        let mut recents = Recents::in_memory();
        for _ in 0..12 {
            let dir = TestDir::new();
            recents.touch(dir.path());
        }
        assert_eq!(recents.list().len(), MAX_RECENTS);
        // Touching the same path again moves it to the front without duplicating.
        let first = recents.list()[0].path.clone();
        recents.touch(Path::new(&first));
        assert_eq!(recents.list().len(), MAX_RECENTS);
        assert_eq!(recents.list()[0].path, first);
    }

    #[test]
    fn recents_persist_and_round_trip() {
        let dir = TestDir::new();
        let file = dir.child("recents.json");
        let repo = TestDir::new();
        {
            let mut recents = Recents::open_at(file.clone());
            recents.touch(repo.path());
        }
        let recents = Recents::open_at(file);
        assert_eq!(recents.list().len(), 1);
        assert_eq!(recents.list()[0].path, repo.path().display().to_string());
        assert!(!recents.list()[0].last_opened_at.is_empty());
    }

    #[test]
    fn recents_tolerate_a_corrupt_file() {
        let dir = TestDir::new();
        let file = dir.child("recents.json");
        std::fs::write(&file, "this is not json").unwrap();
        let recents = Recents::open_at(file);
        assert!(recents.list().is_empty());
    }

    #[test]
    fn recents_remove_deletes_one_entry() {
        let a = TestDir::new();
        let b = TestDir::new();
        let mut recents = Recents::in_memory();
        recents.touch(a.path());
        recents.touch(b.path());
        recents.remove(a.path());
        assert_eq!(recents.list().len(), 1);
        assert_eq!(recents.list()[0].path, b.path().display().to_string());
    }

    #[test]
    fn recent_repositories_mark_missing_dirs() {
        let existing = TestDir::new();
        let missing = TestDir::new().child("gone"); // does not exist
        let mut recents = Recents::in_memory();
        recents.touch(existing.path());
        recents.touch(&missing);

        let list: Vec<RecentRepository> = recents.list().iter().map(recent_repository).collect();
        assert_eq!(list.len(), 2);
        let existing_path = existing.path().display().to_string();
        let missing_path = missing.display().to_string();
        assert!(
            list.iter()
                .find(|r| r.path == existing_path)
                .unwrap()
                .exists
        );
        assert!(!list.iter().find(|r| r.path == missing_path).unwrap().exists);
    }

    #[test]
    fn select_session_restores_the_transcript() {
        let repo = TestDir::new();
        std::fs::create_dir_all(repo.child(".git")).unwrap();
        let (store, _dir) = test_store();

        // A session with a saved conversation.
        let saved = store.create(repo.path()).unwrap();
        saved.append(&user("fix the tests")).unwrap();
        saved
            .append(&Message::Assistant(vava_core::AssistantMessage::new(
                "done",
            )))
            .unwrap();

        // The active session (as opened by the desktop) starts elsewhere.
        let mut session =
            CodingSession::open_with_store(Arc::new(FakeClient), repo.path(), store.clone())
                .unwrap();

        let view = select_session_inner(&mut session, &store, saved.id().as_str()).unwrap();
        assert_eq!(view.session.id, saved.id().as_str());
        assert_eq!(
            view.session.first_user_message.as_deref(),
            Some("fix the tests")
        );
        assert_eq!(view.messages.len(), 2);
        assert!(matches!(view.messages[0], DesktopMessage::User { .. }));
        assert!(matches!(view.messages[1], DesktopMessage::Assistant { .. }));
        // Future messages now append to the selected session.
        assert_eq!(session.session_id(), saved.id());
    }

    #[test]
    fn select_session_with_unknown_id_is_a_session_error() {
        let repo = TestDir::new();
        std::fs::create_dir_all(repo.child(".git")).unwrap();
        let (store, _dir) = test_store();
        let mut session =
            CodingSession::open_with_store(Arc::new(FakeClient), repo.path(), store.clone())
                .unwrap();

        let err = select_session_inner(&mut session, &store, "does-not-exist").unwrap_err();
        assert!(matches!(err, DesktopError::Session(_)));
    }

    #[test]
    fn new_session_starts_fresh_and_preserves_the_repository() {
        let repo = TestDir::new();
        std::fs::create_dir_all(repo.child(".git")).unwrap();
        let (store, _dir) = test_store();

        let mut session =
            CodingSession::open_with_store(Arc::new(FakeClient), repo.path(), store.clone())
                .unwrap();
        // Use the opened session, then switch to a fresh one.
        let first_id = session.session_id().as_str().to_string();
        let view = new_session_inner(&mut session).unwrap();

        assert_ne!(view.session.id, first_id);
        assert!(view.messages.is_empty());
        // Repository context is preserved.
        assert_eq!(session.root(), repo.path());
        // The previous session still exists in the store.
        assert!(store.load(&SessionId::new(first_id)).is_ok());
    }

    /// A repository with a `.git` marker plus a state wired to it.
    async fn opened_state(
        store: SessionStore,
        client: Arc<dyn ModelClient>,
    ) -> (DesktopState, String) {
        let repo = TestDir::new();
        std::fs::create_dir_all(repo.child(".git")).unwrap();
        let state = DesktopState::for_test(store, Recents::in_memory(), Some(client));
        let info = state
            .open_repository(repo.path().to_str().unwrap())
            .await
            .unwrap();
        let session_id = info
            .active_session_id
            .expect("a session is created at open");
        (state, session_id)
    }

    #[tokio::test]
    async fn run_turn_streams_events_and_returns_the_session() {
        let (store, _dir) = test_store();
        let client = Arc::new(ScriptedClient::new(vec![
            ModelEvent::TextDelta("Hello ".into()),
            ModelEvent::TextDelta("world".into()),
            ModelEvent::Finished,
        ]));
        let (state, session_id) = opened_state(store.clone(), client).await;

        let start = state.begin_turn(&session_id).await.unwrap();
        assert!(
            !state.active_session_present().await,
            "session is out during the turn"
        );
        let (event_tx, mut event_rx) = mpsc::channel(64);
        let mut completed_rx = start.completed.subscribe();
        let inner = state.active_handle();
        tokio::spawn(run_turn(
            inner,
            start.session,
            "hello".into(),
            start.token,
            start.id,
            start.completed,
            event_tx,
        ));

        // Collect events until the turn completes.
        let mut saw_turn_started = false;
        let mut text = String::new();
        let mut saw_completed = false;
        while let Some(event) = event_rx.recv().await {
            match &event {
                AgentEvent::TurnStarted => saw_turn_started = true,
                AgentEvent::TextDelta { delta } => text.push_str(delta),
                AgentEvent::AssistantMessageCompleted { message } => {
                    saw_completed = true;
                    assert_eq!(message.content, "Hello world");
                }
                AgentEvent::TurnCompleted => break,
                _ => {}
            }
        }
        assert!(saw_turn_started);
        assert_eq!(text, "Hello world");
        assert!(saw_completed);

        // The task wrote the session back and signaled completion.
        let _ = completed_rx.changed().await;
        assert!(state.active_session_present().await);
        // The turn was persisted to the session log.
        let loaded = store.load(&SessionId::new(&session_id)).unwrap();
        assert_eq!(loaded.messages.len(), 2); // user + assistant
    }

    #[tokio::test]
    async fn stop_running_turn_cancels_and_returns_the_session() {
        let (store, _dir) = test_store();
        let (state, session_id) = opened_state(store, Arc::new(PendingClient)).await;

        let start = state.begin_turn(&session_id).await.unwrap();
        let (event_tx, _event_rx) = mpsc::channel(64);
        let inner = state.active_handle();
        tokio::spawn(run_turn(
            inner,
            start.session,
            "never ends".into(),
            start.token,
            start.id,
            start.completed,
            event_tx,
        ));

        // Give the task a moment to start, then stop the turn.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(!state.active_session_present().await);
        state.stop_running_turn().await.unwrap();
        assert!(
            state.active_session_present().await,
            "session is back after stop"
        );
        // The session is still usable for the next turn.
        let start = state.begin_turn(&session_id).await.unwrap();
        assert_eq!(start.session.session_id().as_str(), session_id);
    }

    #[tokio::test]
    async fn begin_turn_rejects_a_stale_session_id() {
        let (store, _dir) = test_store();
        let client = Arc::new(FakeClient);
        let (state, session_id) = opened_state(store, client).await;

        let err = match state.begin_turn("not-the-active-session").await {
            Ok(_) => panic!("expected a session error"),
            Err(err) => err,
        };
        assert!(matches!(err, DesktopError::Session(_)));
        // The session was not lost.
        assert!(state.active_session_present().await);
        // And the real id still works.
        assert!(state.begin_turn(&session_id).await.is_ok());
    }
}
