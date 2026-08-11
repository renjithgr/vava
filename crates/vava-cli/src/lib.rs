//! The vava command-line frontend: argument parsing, wiring, and rendering.
//!
//! This layer is responsible only for interaction: it turns command-line
//! arguments into a configured agent, runs it, and renders the [`AgentEvent`]s
//! it receives. It knows nothing about SSE framing or the DeepSeek wire
//! protocol.

pub mod render;
pub mod repl;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use secrecy::SecretString;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

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

/// Wire everything together and run in print or REPL mode.
pub async fn run(cli: Cli) -> anyhow::Result<()> {
    let api_key = std::env::var("DEEPSEEK_API_KEY")
        .context("DEEPSEEK_API_KEY is not set (export it before running vava)")?;

    let config = ModelConfig::new(cli.model.clone()).with_thinking(cli.effective_thinking());
    let client = DeepSeekClient::new(SecretString::from(api_key), config);

    let root = cli.cwd.clone().unwrap_or(std::env::current_dir()?);
    let client: Arc<dyn vava_core::ModelClient> = Arc::new(client);
    let session = vava_coding::CodingSession::open(client, &root)?;

    match cli.prompt.clone() {
        Some(prompt) => run_print(session, prompt, cli.debug).await,
        None => repl::run(session, &cli.model, cli.debug).await,
    }
}

/// One-shot print mode: run the prompt, render events, exit.
async fn run_print(
    mut session: vava_coding::CodingSession,
    prompt: String,
    debug: bool,
) -> anyhow::Result<()> {
    // Ctrl-C cancels the in-flight turn.
    let token = CancellationToken::new();
    let ctrl_c = tokio::spawn({
        let token = token.clone();
        async move {
            let _ = tokio::signal::ctrl_c().await;
            token.cancel();
        }
    });

    let (tx, rx) = mpsc::channel(64);
    let renderer = tokio::spawn(render::render_events(rx, debug, Some(prompt.clone())));

    let result = session.prompt(prompt, tx, token).await;
    ctrl_c.abort();
    renderer.await.ok();
    result.map_err(anyhow::Error::from)
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
}
