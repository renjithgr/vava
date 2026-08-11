//! The vava command-line frontend: argument parsing, wiring, and rendering.
//!
//! This layer is responsible only for interaction: it turns command-line
//! arguments into a configured agent, runs it, and renders the [`AgentEvent`]s
//! it receives. It knows nothing about SSE framing or the DeepSeek wire
//! protocol. Session discovery and loading live in `vava-coding`.

pub mod render;
pub mod repl;
pub mod session_ui;
pub mod tui;

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use secrecy::SecretString;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use vava_coding::{CodingSession, PrefixMatch, SessionStore};
use vava_deepseek::{DeepSeekClient, ModelConfig};

use crate::session_ui::{choose_session, listing_lines, resumed_banner};

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
    /// Omitting it enters the interactive REPL (or the TUI with `--tui`).
    #[arg(short, long, conflicts_with = "resume")]
    pub prompt: Option<String>,

    /// Continue the most recent session for this repository.
    #[arg(short = 'c', long = "continue", conflicts_with = "resume")]
    pub continue_session: bool,

    /// Resume an existing session for this repository.
    ///
    /// With no value, lists the sessions and asks which one to resume; with
    /// an id or unique prefix, resumes that session directly.
    #[arg(
        short = 'r',
        long = "resume",
        num_args = 0..=1,
        conflicts_with_all = ["continue_session", "prompt"]
    )]
    pub resume: Option<Option<String>>,

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

    /// Use the full-screen terminal UI instead of the plain REPL.
    #[arg(long)]
    pub tui: bool,
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

/// Which session the invocation should attach to.
enum SessionStart {
    /// A brand-new session.
    Fresh,
    /// The most recent session of this repository (or a new one if none).
    Continue,
    /// The interactive picker (`vava -r`).
    Pick,
    /// An explicit id or unique prefix (`vava --resume <id>`).
    Id(String),
}

/// Wire everything together and run in print or REPL mode.
pub async fn run(cli: Cli) -> anyhow::Result<()> {
    let api_key = std::env::var("DEEPSEEK_API_KEY")
        .context("DEEPSEEK_API_KEY is not set (export it before running vava)")?;

    let config = ModelConfig::new(cli.model.clone()).with_thinking(cli.effective_thinking());
    let client = DeepSeekClient::new(SecretString::from(api_key), config);
    let client: Arc<dyn vava_core::ModelClient> = Arc::new(client);

    let start = cli.cwd.clone().unwrap_or(std::env::current_dir()?);
    let root = vava_coding::ProjectContext::discover(&start)?.root;
    let store = SessionStore::open()?;

    let mode = if let Some(resume) = cli.resume.clone() {
        match resume {
            Some(id) => SessionStart::Id(id),
            None => SessionStart::Pick,
        }
    } else if cli.continue_session {
        SessionStart::Continue
    } else {
        SessionStart::Fresh
    };

    let Some((session, banner)) = start_session(mode, client, &root, store, cli.prompt.is_none())?
    else {
        return Ok(()); // resume target not found / picker cancelled
    };
    if let Some(banner) = banner {
        println!("{banner}");
    }

    match cli.prompt.clone() {
        Some(prompt) => run_print(session, prompt, cli.debug).await,
        None if cli.tui => tui::run(session, &cli.model, cli.debug).await,
        None => repl::run(session, &cli.model, cli.debug).await,
    }
}

/// Resolve the requested session mode into a running `CodingSession` and
/// an optional one-line banner to print, or `None` when the mode was a
/// resume request that could not be satisfied (already reported to the
/// user — the CLI returns cleanly). Repository scoping happens here:
/// listing, prefix matching, and loading all go through the store for this
/// repository's root.
fn start_session(
    mode: SessionStart,
    client: Arc<dyn vava_core::ModelClient>,
    root: &std::path::Path,
    store: SessionStore,
    interactive: bool,
) -> anyhow::Result<Option<(CodingSession, Option<String>)>> {
    match mode {
        SessionStart::Fresh => {
            let session = CodingSession::open_with_store(client, root, store)?;
            let banner =
                interactive.then(|| format!("Started session {}", session.session_id().short()));
            Ok(Some((session, banner)))
        }
        SessionStart::Continue => {
            // Newest first; if the most recent session cannot be loaded
            // (e.g. middle-record corruption), fall through to the next
            // valid one. Truncated final lines load fine by design.
            match store.latest_loadable(root)? {
                Some(loaded) => {
                    let summary = loaded.summary.clone();
                    let session = CodingSession::resume_with_store(client, root, store, loaded)?;
                    Ok(Some((session, Some(resumed_banner(&summary)))))
                }
                None => {
                    let session = CodingSession::open_with_store(client, root, store)?;
                    Ok(Some((
                        session,
                        Some("No previous session found; started a new session.".into()),
                    )))
                }
            }
        }
        SessionStart::Pick => {
            let sessions = store.list_for_repository(root)?;
            if sessions.is_empty() {
                println!("No sessions found for this repository.");
                return Ok(None);
            }
            for line in listing_lines(&sessions, root) {
                println!("{line}");
            }
            print!("Select session: ");
            std::io::stdout().flush().context("flush stdout")?;
            let mut input = String::new();
            std::io::stdin().read_line(&mut input)?;
            match choose_session(&input, &sessions) {
                session_ui::PickResult::Picked(summary) => {
                    let loaded = store.load(&summary.id)?;
                    let session = CodingSession::resume_with_store(client, root, store, loaded)?;
                    Ok(Some((session, Some(resumed_banner(summary)))))
                }
                session_ui::PickResult::Cancelled => {
                    println!("No session selected.");
                    Ok(None)
                }
                session_ui::PickResult::Ambiguous(matches) => {
                    println!("Session prefix is ambiguous.");
                    for summary in matches {
                        println!(
                            "  {}  {:?}",
                            summary.id.short(),
                            summary.first_user_message.as_deref().unwrap_or("")
                        );
                    }
                    Ok(None)
                }
            }
        }
        SessionStart::Id(id) => {
            let sessions = store.list_for_repository(root)?;
            match vava_coding::resolve_prefix(&sessions, &id) {
                PrefixMatch::Unique(summary) => {
                    let loaded = store.load(&summary.id)?;
                    let session = CodingSession::resume_with_store(client, root, store, loaded)?;
                    Ok(Some((session, Some(resumed_banner(summary)))))
                }
                PrefixMatch::Ambiguous(matches) => {
                    eprintln!("Session prefix is ambiguous.");
                    for summary in matches {
                        eprintln!(
                            "  {}  {:?}",
                            summary.id.short(),
                            summary.first_user_message.as_deref().unwrap_or("")
                        );
                    }
                    Ok(None)
                }
                PrefixMatch::None => {
                    eprintln!("Session not found for this repository.");
                    Ok(None)
                }
            }
        }
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
        assert!(!cli.continue_session);
        assert!(cli.resume.is_none());
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
    fn continue_short_and_long_flags() {
        let short = Cli::try_parse_from(["vava", "-c"]).unwrap();
        assert!(short.continue_session);
        let long = Cli::try_parse_from(["vava", "--continue"]).unwrap();
        assert!(long.continue_session);
    }

    #[test]
    fn resume_without_a_value_opens_the_picker() {
        let short = Cli::try_parse_from(["vava", "-r"]).unwrap();
        assert_eq!(short.resume, Some(None));
        let long = Cli::try_parse_from(["vava", "--resume"]).unwrap();
        assert_eq!(long.resume, Some(None));
    }

    #[test]
    fn resume_with_an_id_or_prefix() {
        let cli = Cli::try_parse_from(["vava", "--resume", "01KABC"]).unwrap();
        assert_eq!(cli.resume, Some(Some("01KABC".into())));
        assert!(!cli.continue_session);
    }

    #[test]
    fn continue_conflicts_with_resume() {
        let err = Cli::try_parse_from(["vava", "-c", "--resume", "01KABC"]).unwrap_err();
        assert!(err.to_string().contains("cannot be used with"));
        let err = Cli::try_parse_from(["vava", "-c", "-r"]).unwrap_err();
        assert!(err.to_string().contains("cannot be used with"));
    }

    #[test]
    fn prompt_conflicts_with_resume() {
        let err = Cli::try_parse_from(["vava", "-p", "hello", "--resume"]).unwrap_err();
        assert!(err.to_string().contains("cannot be used with"));
        let err = Cli::try_parse_from(["vava", "-p", "hello", "--resume", "01KABC"]).unwrap_err();
        assert!(err.to_string().contains("cannot be used with"));
    }

    #[test]
    fn continue_is_compatible_with_a_prompt() {
        // "continue the latest session and run this one prompt" is allowed.
        let cli = Cli::try_parse_from(["vava", "-c", "-p", "run the tests"]).unwrap();
        assert!(cli.continue_session);
        assert_eq!(cli.prompt.as_deref(), Some("run the tests"));
    }
}
