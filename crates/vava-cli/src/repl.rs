//! The interactive REPL.
//!
//! `vava` with no prompt enters a loop: read a line, run it through the
//! session, render the events, repeat. The transcript and the session log
//! accumulate across turns.
//!
//! Slash commands:
//! - `/new` starts a fresh session for the same repository
//! - `/session` shows the current session's metadata
//! - `/resume` lists this repository's sessions and switches to one
//! - `quit` / `exit`, or Ctrl-D at a prompt, exits
//!
//! - Ctrl-C while a turn is running cancels it
//! - Ctrl-C while idle exits; while choosing a session it cancels the choice

use std::io::{BufRead, Write};

use anyhow::Context;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use vava_coding::{CodingSession, SessionSummary};
use vava_core::AgentError;

use crate::render;
use crate::session_ui::{self, PickResult};

/// The REPL's small state machine: normally waiting for a prompt, or
/// awaiting a session choice after `/resume`.
enum Mode {
    Prompt,
    ChooseSession { sessions: Vec<SessionSummary> },
}

/// Run the interactive REPL.
pub async fn run(
    mut session: CodingSession,
    model: &str,
    show_reasoning: bool,
) -> anyhow::Result<()> {
    println!("vava — {model}");
    println!("Ctrl-C cancels the current turn; Ctrl-C while idle exits.\n");

    // Stdin is read on a blocking thread so the async runtime never blocks
    // on input (which would also stall Ctrl-C handling).
    let (line_tx, mut line_rx) = mpsc::channel::<String>(16);
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        loop {
            let mut line = String::new();
            match stdin.read_line(&mut line) {
                Ok(0) => break, // EOF (Ctrl-D)
                Ok(_) => {
                    if line_tx.blocking_send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let mut mode = Mode::Prompt;
    loop {
        match &mode {
            Mode::Prompt => print!("> "),
            Mode::ChooseSession { .. } => print!("session # or id (empty to cancel): "),
        }
        std::io::stdout().flush().context("flush stdout")?;

        // Idle: wait for input; Ctrl-C exits (or cancels a session choice).
        let line = tokio::select! {
            line = line_rx.recv() => match line {
                Some(line) => line,
                None => break, // stdin closed
            },
            _ = tokio::signal::ctrl_c() => {
                match mode {
                    Mode::Prompt => break,
                    Mode::ChooseSession { .. } => {
                        println!();
                        mode = Mode::Prompt;
                        continue;
                    }
                }
            }
        };

        let input = line.trim().to_string();
        if input.is_empty() {
            if matches!(mode, Mode::ChooseSession { .. }) {
                println!("No session selected.");
                mode = Mode::Prompt;
            }
            continue;
        }

        let previous = std::mem::replace(&mut mode, Mode::Prompt);
        match previous {
            Mode::Prompt => {
                if is_exit_command(&input) {
                    break;
                }
                match input.as_str() {
                    "/new" => match session.begin_new_session() {
                        Ok(summary) => println!("Started new session {}", summary.id.short()),
                        Err(error) => println!("error: {error}"),
                    },
                    "/session" => {
                        for line in session_ui::info_lines(&session) {
                            println!("{line}");
                        }
                    }
                    "/resume" => {
                        match session.session_store().list_for_repository(session.root()) {
                            Ok(sessions) if sessions.is_empty() => {
                                println!("No sessions found for this repository.");
                            }
                            Ok(sessions) => {
                                for line in session_ui::listing_lines(&sessions, session.root()) {
                                    println!("{line}");
                                }
                                mode = Mode::ChooseSession { sessions };
                            }
                            Err(error) => println!("error: {error}"),
                        }
                    }
                    _ => run_turn(&mut session, input, show_reasoning).await,
                }
            }
            Mode::ChooseSession { sessions } => match session_ui::choose_session(&input, &sessions)
            {
                PickResult::Picked(summary) => match session.session_store().load(&summary.id) {
                    Ok(loaded) => match session.resume_into(loaded) {
                        Ok(()) => println!("Switched to {}", summary.id.short()),
                        Err(error) => println!("error: {error}"),
                    },
                    Err(error) => println!("error: {error}"),
                },
                PickResult::Cancelled => println!("No session selected."),
                PickResult::Ambiguous(matches) => {
                    println!("Session prefix is ambiguous.");
                    for summary in matches {
                        println!(
                            "  {}  {:?}",
                            summary.id.short(),
                            summary.first_user_message.as_deref().unwrap_or("")
                        );
                    }
                }
            },
        }
    }
    Ok(())
}

/// Run one prompt to completion; Ctrl-C cancels it.
async fn run_turn(session: &mut CodingSession, prompt: String, show_reasoning: bool) {
    // A fresh token per turn, so a cancelled turn never poisons the next.
    let token = CancellationToken::new();
    let ctrl_c = tokio::spawn({
        let token = token.clone();
        async move {
            let _ = tokio::signal::ctrl_c().await;
            token.cancel();
        }
    });

    let (tx, rx) = mpsc::channel(64);
    let renderer = tokio::spawn(render::render_events(rx, show_reasoning, None));

    let result = session.prompt(prompt, tx, token).await;
    ctrl_c.abort();
    renderer.await.ok();

    // The renderer already showed the failure via `AgentEvent::Error`;
    // only cancellation gets an extra, friendlier confirmation.
    match result {
        Ok(()) => {}
        Err(AgentError::Cancelled) => println!("(cancelled)\n"),
        Err(_) => {}
    }
}

/// Whether a line is a REPL exit command.
fn is_exit_command(line: &str) -> bool {
    matches!(line, "quit" | "exit")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_exit_commands() {
        assert!(is_exit_command("quit"));
        assert!(is_exit_command("exit"));
        assert!(!is_exit_command("exit the program"));
        assert!(!is_exit_command("run cargo test"));
    }
}
