//! The coding tools: `read`, `write`, `edit`, `bash`.
//!
//! All filesystem tools resolve paths against the workspace root and refuse
//! to escape it:
//!
//! ```text
//! requested path
//!       ↓
//! resolve against root
//!       ↓
//! normalize / canonicalize
//!       ↓
//! verify under root
//!       ↓
//! operate
//! ```
//!
//! The `bash` tool is intentionally *not* sandboxed: it runs commands with
//! the user's permissions. See the README's security model.

mod read;

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use thiserror::Error;

use vava_core::ToolRegistry;

/// Register every coding tool into a registry.
///
/// `CodingSession` calls this when a session starts; the CLI used it
/// directly until that layer lands.
pub fn register_coding_tools(registry: &mut ToolRegistry) {
    registry.register(Arc::new(read::ReadTool));
}

/// Errors from resolving a tool path against the workspace root.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PathError {
    /// The resolved path escapes the workspace root.
    #[error("path `{0}` is outside the workspace root")]
    OutsideRoot(String),
    /// The path is not usable.
    #[error("path `{0}` is invalid: {1}")]
    Invalid(String, String),
}

/// Resolve `requested` against `root` and verify the result stays inside
/// the root.
///
/// Rules:
/// - relative paths are resolved against the root; absolute paths are used
///   as given (then checked)
/// - `..` components that would escape the root are rejected
/// - symlinks are resolved via canonicalization and re-checked, so a link
///   inside the root pointing outside is rejected
/// - the deepest existing ancestor is canonicalized, so paths that do not
///   exist yet (for `write`) are still validated
pub fn resolve_within_root(root: &Path, requested: &str) -> Result<PathBuf, PathError> {
    if requested.is_empty() {
        return Err(PathError::Invalid(
            "path must not be empty".to_string(),
            "pass a relative or absolute path".to_string(),
        ));
    }

    // Canonicalize the root first, so a root reached through a symlink
    // (e.g. /var -> /private/var on macOS) compares correctly.
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let root_normalized = normalize_path(&root);

    let candidate = if Path::new(requested).is_absolute() {
        PathBuf::from(requested)
    } else {
        root.join(requested)
    };
    let candidate_normalized = normalize_path(&candidate);

    // Lexical check first: cheap, and rejects obvious escapes.
    if !candidate_normalized.starts_with(&root_normalized) {
        return Err(PathError::OutsideRoot(requested.to_string()));
    }

    // Canonical check: resolve symlinks in the deepest existing ancestor.
    let resolved = canonicalize_deepest_existing(&candidate_normalized);
    if !resolved.starts_with(&root_normalized) {
        return Err(PathError::OutsideRoot(requested.to_string()));
    }
    Ok(resolved)
}

/// Remove `.` components and apply `..` lexically.
fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Canonicalize the deepest existing ancestor of `path`, appending the
/// remaining components unchanged.
fn canonicalize_deepest_existing(path: &Path) -> PathBuf {
    let mut current = path.to_path_buf();
    let mut suffix: Vec<std::ffi::OsString> = Vec::new();
    loop {
        match std::fs::canonicalize(&current) {
            Ok(resolved) => {
                let mut out = resolved;
                for component in suffix.iter().rev() {
                    out.push(component);
                }
                return out;
            }
            Err(_) => match current.file_name() {
                Some(name) => {
                    suffix.push(name.to_os_string());
                    current.pop();
                }
                None => return path.to_path_buf(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique temporary directory that cleans itself up on drop.
    struct TestDir(PathBuf);

    impl TestDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("vava-test-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            // Canonicalize so comparisons hold on platforms where the temp
            // dir is reached through a symlink (e.g. /var -> /private/var).
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

    #[test]
    fn resolves_relative_paths_inside_the_root() {
        let dir = TestDir::new("resolve-in");
        std::fs::create_dir_all(dir.child("src")).unwrap();
        let resolved = resolve_within_root(dir.path(), "src/main.rs").unwrap();
        assert_eq!(resolved, dir.child("src/main.rs"));
    }

    #[test]
    fn resolves_absolute_paths_inside_the_root() {
        let dir = TestDir::new("resolve-abs");
        let resolved =
            resolve_within_root(dir.path(), dir.child("Cargo.toml").to_str().unwrap()).unwrap();
        assert_eq!(resolved, dir.child("Cargo.toml"));
    }

    #[test]
    fn rejects_parent_traversal_outside_the_root() {
        let dir = TestDir::new("resolve-parent");
        let err = resolve_within_root(dir.path(), "../secret.txt").unwrap_err();
        assert!(matches!(err, PathError::OutsideRoot(_)));

        let err = resolve_within_root(dir.path(), "a/../../secret.txt").unwrap_err();
        assert!(matches!(err, PathError::OutsideRoot(_)));
    }

    #[test]
    fn rejects_absolute_paths_outside_the_root() {
        let dir = TestDir::new("resolve-outside");
        let err = resolve_within_root(dir.path(), "/etc/passwd").unwrap_err();
        assert!(matches!(err, PathError::OutsideRoot(_)));
    }

    #[test]
    fn rejects_empty_paths() {
        let dir = TestDir::new("resolve-empty");
        assert!(matches!(
            resolve_within_root(dir.path(), ""),
            Err(PathError::Invalid(..))
        ));
    }

    #[test]
    fn allows_nonexistent_paths_inside_the_root() {
        // `write` needs to create files that do not exist yet.
        let dir = TestDir::new("resolve-new");
        let resolved = resolve_within_root(dir.path(), "new/dir/file.rs").unwrap();
        assert_eq!(resolved, dir.child("new/dir/file.rs"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks_that_escape_the_root() {
        use std::os::unix::fs::symlink;

        let inside = TestDir::new("resolve-symlink-in");
        let outside = TestDir::new("resolve-symlink-out");
        std::fs::write(outside.child("secret.txt"), "secret").unwrap();
        symlink(outside.child("secret.txt"), inside.child("link.txt")).unwrap();

        let err = resolve_within_root(inside.path(), "link.txt").unwrap_err();
        assert!(matches!(err, PathError::OutsideRoot(_)));
    }

    #[test]
    fn normalizes_dot_components() {
        let dir = TestDir::new("resolve-dot");
        let resolved = resolve_within_root(dir.path(), "./a/./b.txt").unwrap();
        assert_eq!(resolved, dir.child("a/b.txt"));
    }

    #[test]
    fn register_adds_the_read_tool() {
        let mut registry = ToolRegistry::new();
        register_coding_tools(&mut registry);
        assert_eq!(registry.names(), vec!["read"]);
    }
}
