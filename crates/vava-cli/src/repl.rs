//! The interactive REPL.
//!
//! `vava` with no prompt enters a loop: read a line, run it through the
//! session, render the events, repeat. The transcript and the session log
//! accumulate across turns.
//!
//! - Ctrl-C while a turn is running cancels it
//! - Ctrl-C while idle exits
//! - `quit` / `exit`, or Ctrl-D at a prompt, exits

use std::io::{BufRead, Write};

use anyhow::Context;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use vava_coding::CodingSession;
use vava_core::AgentError;

use crate::render;

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

    loop {
        print!("> ");
        std::io::stdout().flush().context("flush stdout")?;

        // Idle: wait for input; Ctrl-C exits.
        let line = tokio::select! {
            line = line_rx.recv() => match line {
                Some(line) => line,
                None => break, // stdin closed
            },
            _ = tokio::signal::ctrl_c() => break,
        };

        let prompt = line.trim().to_string();
        if prompt.is_empty() {
            continue;
        }
        if is_exit_command(&prompt) {
            break;
        }

        run_turn(&mut session, prompt, show_reasoning).await;
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
