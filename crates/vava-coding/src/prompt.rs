//! System prompt construction.

use std::path::Path;

/// The default coding system prompt, with project instructions appended.
///
/// The repository tree is deliberately not injected, and no files are
/// pre-read: the model discovers repository context through the tools.
pub fn system_prompt(root: &Path, project_instructions: Option<&str>) -> String {
    let mut prompt = format!(
        "You are vava, a coding agent operating inside a software repository.\n\n\
         Working directory:\n{}\n\n\
         Use the provided tools to inspect and modify the repository.\n\n\
         Guidelines:\n\
         - Inspect relevant code before modifying it.\n\
         - Do not invent file contents.\n\
         - Prefer minimal, focused changes.\n\
         - Follow existing project conventions.\n\
         - Run relevant tests after making changes.\n\
         - If a tool fails, inspect the error and adjust your approach.\n\
         - Use tools whenever repository information is required.",
        root.display()
    );
    if let Some(instructions) = project_instructions {
        prompt.push_str("\n\nProject instructions:\n");
        prompt.push_str(instructions);
    }
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn includes_the_working_directory() {
        let prompt = system_prompt(&PathBuf::from("/projects/foo"), None);
        assert!(prompt.contains("/projects/foo"));
        assert!(prompt.contains("You are vava, a coding agent"));
    }

    #[test]
    fn appends_project_instructions() {
        let prompt = system_prompt(&PathBuf::from("/p"), Some("Run cargo test.\n"));
        assert!(prompt.ends_with("Project instructions:\nRun cargo test.\n"));
    }

    #[test]
    fn omits_the_instructions_section_when_absent() {
        let prompt = system_prompt(&PathBuf::from("/p"), None);
        assert!(!prompt.contains("Project instructions"));
    }

    #[test]
    fn does_not_inject_the_repository_tree() {
        let prompt = system_prompt(&PathBuf::from("/p"), None);
        assert!(!prompt.contains("file tree") && !prompt.contains("tree:"));
    }
}
