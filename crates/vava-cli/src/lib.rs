//! The vava command-line frontend: argument parsing, wiring, and rendering.
//!
//! This layer is responsible only for interaction: it turns command-line
//! arguments into a configured agent, runs it, and renders the [`AgentEvent`]s
//! it receives. It knows nothing about SSE framing or the DeepSeek wire
//! protocol.

pub mod render;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use secrecy::SecretString;
use tokio::sync::mpsc;

use vava_core::{AgentHarness, ToolRegistry};
use vava_deepseek::{DeepSeekClient, ModelConfig};

/// Command-line arguments for `vava`.
#[derive(Debug, Parser)]
#[command(
    name = "vava",
    version,
    about = "A minimal Rust coding agent powered by DeepSeek"
)]
pub struct Cli {
    /// The prompt to run.
    ///
    /// Omitting it will eventually enter the interactive REPL.
    #[arg(short, long)]
    pub prompt: Option<String>,

    /// Working directory (defaults to the current directory).
    #[arg(long)]
    pub cwd: Option<PathBuf>,

    /// The DeepSeek model to use.
    #[arg(long, default_value = vava_deepseek::DEFAULT_MODEL)]
    pub model: String,

    /// Enable thinking (reasoning) mode.
    #[arg(long, conflicts_with = "no_thinking")]
    pub thinking: bool,

    /// Disable thinking mode explicitly.
    #[arg(long)]
    pub no_thinking: bool,

    /// Show reasoning content in the output.
    #[arg(long)]
    pub debug: bool,
}

impl Cli {
    /// The effective thinking setting: `--thinking` wins over `--no-thinking`.
    fn effective_thinking(&self) -> bool {
        if self.no_thinking {
            false
        } else {
            self.thinking
        }
    }
}

/// Wire everything together and execute the prompt.
pub async fn run(cli: Cli) -> anyhow::Result<()> {
    let Some(prompt) = cli.prompt.clone() else {
        anyhow::bail!("interactive mode is not implemented yet; pass -p/--prompt");
    };

    let api_key = std::env::var("DEEPSEEK_API_KEY")
        .context("DEEPSEEK_API_KEY is not set (export it before running vava)")?;

    let config = ModelConfig::new(cli.model.clone()).with_thinking(cli.effective_thinking());
    let client = DeepSeekClient::new(SecretString::from(api_key), config);

    let root = cli.cwd.clone().unwrap_or(std::env::current_dir()?);
    let system = system_prompt(&root);

    let mut registry = ToolRegistry::new();
    vava_coding::tools::register_coding_tools(&mut registry);

    let mut harness = AgentHarness::new(Arc::new(client), registry, system, root);

    // Ctrl-C cancels the in-flight turn.
    let token = harness.cancellation_token();
    let ctrl_c = tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        token.cancel();
    });

    let (tx, rx) = mpsc::channel(64);
    let renderer = tokio::spawn(render::render_events(rx, cli.debug));

    let result = harness.prompt(prompt, tx).await;
    ctrl_c.abort();
    renderer.await.ok();
    result.map_err(anyhow::Error::from)
}

/// The default system prompt.
///
/// Repository-specific instructions (`AGENTS.md`) are added by the coding
/// session in a later milestone; this is the generic baseline.
fn system_prompt(root: &std::path::Path) -> String {
    format!(
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
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_invocation() {
        let cli = Cli::try_parse_from(["vava", "-p", "say hello"]).unwrap();
        assert_eq!(cli.prompt.as_deref(), Some("say hello"));
        assert_eq!(cli.model, vava_deepseek::DEFAULT_MODEL);
        assert!(!cli.thinking);
        assert!(!cli.no_thinking);
    }

    #[test]
    fn parses_model_and_thinking_flags() {
        let cli = Cli::try_parse_from([
            "vava",
            "-p",
            "hi",
            "--model",
            "deepseek-reasoner",
            "--thinking",
            "--debug",
        ])
        .unwrap();
        assert_eq!(cli.model, "deepseek-reasoner");
        assert!(cli.effective_thinking());
        assert!(cli.debug);
    }

    #[test]
    fn thinking_conflicts_with_no_thinking() {
        let err =
            Cli::try_parse_from(["vava", "-p", "hi", "--thinking", "--no-thinking"]).unwrap_err();
        assert!(err.to_string().contains("cannot be used with"));
    }

    #[test]
    fn no_thinking_disables_thinking() {
        let cli = Cli::try_parse_from(["vava", "-p", "hi", "--no-thinking"]).unwrap();
        assert!(!cli.effective_thinking());
    }

    #[test]
    fn system_prompt_contains_the_working_directory() {
        let prompt = system_prompt(&PathBuf::from("/projects/foo"));
        assert!(prompt.contains("/projects/foo"));
        assert!(prompt.contains("coding agent"));
    }
}
