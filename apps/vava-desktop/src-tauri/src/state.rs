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
use tokio::sync::Mutex as AsyncMutex;

use vava_coding::{CodingSession, ProjectContext, SessionStore, SessionSummary};
use vava_core::ModelClient;
use vava_deepseek::{DeepSeekClient, ModelConfig};

use crate::errors::DesktopError;
use crate::model::{RecentRepository, RepositoryInfo, SessionInfo};

/// How many recent repositories to keep.
const MAX_RECENTS: usize = 10;

/// Application-wide state managed by Tauri and handed to every command.
pub struct DesktopState {
    /// The currently open repository, if any.
    active: AsyncMutex<Option<ActiveRepository>>,
    /// The persisted recent-repository list.
    recents: std::sync::Mutex<Recents>,
}

/// The open repository and its session machinery.
pub struct ActiveRepository {
    root: PathBuf,
    context: ProjectContext,
    /// The active `CodingSession`, when a model client is available.
    /// `None` while the repository is browsable but not yet prompt-ready
    /// (no API key configured — D9 adds desktop settings).
    session: Option<CodingSession>,
}

impl Default for DesktopState {
    fn default() -> Self {
        Self::new()
    }
}

impl DesktopState {
    /// Create the initial application state.
    pub fn new() -> Self {
        Self {
            active: AsyncMutex::new(None),
            recents: std::sync::Mutex::new(Recents::open()),
        }
    }

    /// Open the repository containing `path`.
    ///
    /// Resolves the workspace root, initializes a [`CodingSession`] when a
    /// model client is available (currently from `DEEPSEEK_API_KEY`, like
    /// the CLI), lists saved sessions, and records the repository in
    /// recents.
    pub async fn open_repository(&self, path: &str) -> Result<RepositoryInfo, DesktopError> {
        let store =
            SessionStore::open().map_err(|error| DesktopError::Session(error.to_string()))?;
        let client = model_client_from_env();
        let opened = {
            let mut recents = self.recents.lock().unwrap();
            open_repository_inner(Path::new(path), &store, &mut recents, client)?
        };
        *self.active.lock().await = Some(ActiveRepository {
            root: opened.context.root.clone(),
            context: opened.context,
            session: opened.session,
        });
        Ok(opened.info)
    }

    /// The currently open repository, if any (sessions re-listed live).
    pub async fn active_repository(&self) -> Result<Option<RepositoryInfo>, DesktopError> {
        let active = self.active.lock().await;
        let Some(active) = active.as_ref() else {
            return Ok(None);
        };
        let store =
            SessionStore::open().map_err(|error| DesktopError::Session(error.to_string()))?;
        let sessions = store
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
}
