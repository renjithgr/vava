//! Project context: repository root discovery and project instructions.
//!
//! When vava starts it takes the supplied directory (or the current one),
//! locates the nearest Git repository root if one exists, and uses that as
//! the workspace boundary for tools. Git is *not* required: a `.git` entry
//! (directory for a clone, file for a worktree/submodule) marks the root.

use std::path::{Path, PathBuf};

use thiserror::Error;

/// Everything the coding session knows about the repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectContext {
    /// The workspace boundary for all tools.
    pub root: PathBuf,
    /// The contents of `<root>/AGENTS.md`, if present.
    pub agents_md: Option<String>,
}

impl ProjectContext {
    /// Discover the repository context starting from `start`:
    ///
    /// 1. the nearest ancestor containing a `.git` entry becomes the root
    /// 2. otherwise `start` itself is the root
    pub fn discover(start: &Path) -> Result<Self, ContextError> {
        let start = std::fs::canonicalize(start)
            .map_err(|_| ContextError::WorkingDir(start.to_path_buf()))?;
        let root = find_repo_root(&start);
        tracing::debug!(root = %root.display(), "discovered repository root");
        let agents_md = load_agents_md(&root)?;
        Ok(Self { root, agents_md })
    }
}

/// Find the nearest repository root above `start`, or `start` itself.
pub fn find_repo_root(start: &Path) -> PathBuf {
    let start = if start.is_absolute() {
        start.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(start))
            .unwrap_or_else(|_| start.to_path_buf())
    };
    let mut current = start.clone();
    loop {
        if current.join(".git").exists() {
            return std::fs::canonicalize(&current).unwrap_or(current);
        }
        match current.parent() {
            Some(parent) => current = parent.to_path_buf(),
            None => return std::fs::canonicalize(&start).unwrap_or(start),
        }
    }
}

/// Load `<root>/AGENTS.md` if it exists.
pub fn load_agents_md(root: &Path) -> Result<Option<String>, ContextError> {
    match std::fs::read_to_string(root.join("AGENTS.md")) {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ContextError::AgentsMd(error)),
    }
}

/// Errors from building the project context.
#[derive(Debug, Error)]
pub enum ContextError {
    /// The starting directory does not exist or cannot be resolved.
    #[error("working directory `{0}` does not exist")]
    WorkingDir(PathBuf),
    /// `AGENTS.md` exists but could not be read.
    #[error("could not read AGENTS.md: {0}")]
    AgentsMd(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_util::TestDir;

    #[test]
    fn repo_root_is_found_from_a_nested_directory() {
        let dir = TestDir::new();
        std::fs::create_dir_all(dir.child(".git")).unwrap();
        std::fs::create_dir_all(dir.child("src/deep/nested")).unwrap();
        assert_eq!(
            find_repo_root(&dir.child("src/deep/nested")),
            dir.path().to_path_buf()
        );
    }

    #[test]
    fn nearest_repo_root_wins() {
        let dir = TestDir::new();
        std::fs::create_dir_all(dir.child(".git")).unwrap();
        std::fs::create_dir_all(dir.child("sub/.git")).unwrap();
        assert_eq!(find_repo_root(&dir.child("sub/x")), dir.child("sub"));
    }

    #[test]
    fn without_a_repo_the_start_is_the_root() {
        let dir = TestDir::new();
        assert_eq!(find_repo_root(dir.path()), dir.path().to_path_buf());
    }

    #[test]
    fn a_git_file_counts_as_a_repo_root() {
        // Worktrees and submodules use a `.git` file, not a directory.
        let dir = TestDir::new();
        std::fs::write(dir.child(".git"), "gitdir: ../real").unwrap();
        std::fs::create_dir_all(dir.child("nested")).unwrap();
        assert_eq!(
            find_repo_root(&dir.child("nested")),
            dir.path().to_path_buf()
        );
    }

    #[test]
    fn discover_loads_agents_md() {
        let dir = TestDir::new();
        dir.write("AGENTS.md", "Always run cargo test.\n");
        let context = ProjectContext::discover(dir.path()).unwrap();
        assert_eq!(context.root, dir.path().to_path_buf());
        assert_eq!(
            context.agents_md.as_deref(),
            Some("Always run cargo test.\n")
        );
    }

    #[test]
    fn discover_without_agents_md() {
        let dir = TestDir::new();
        let context = ProjectContext::discover(dir.path()).unwrap();
        assert_eq!(context.agents_md, None);
    }

    #[test]
    fn discover_rejects_a_missing_directory() {
        let dir = TestDir::new();
        let err = ProjectContext::discover(&dir.child("nope")).unwrap_err();
        assert!(matches!(err, ContextError::WorkingDir(_)));
    }
}
