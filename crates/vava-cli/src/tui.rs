//! A thin Ratatui frontend consuming the same [`AgentEvent`]s as the print
//! renderer.
//!
//! The TUI reuses the exact rendering logic from [`crate::render`]: events
//! are rendered into a text buffer by the same [`Renderer`], and the result
//! is shown as a scrollable conversation panel with an input line below.
//! The core architecture (harness, session, events) is untouched.
//!
//! Layout:
//!
//! ```text
//! ┌─────────────────────────────────────────────────────┐
//! │ vava                            deepseek-chat       │
//! ├─────────────────────────────────────────────────────┤
//! │ > Fix the failing tests                            │
//! │                                                    │
//! │ ● Bash                                             │
//! │   cargo test                                       │
//! │                                                    │
//! │ The issue was caused by...                         │
//! ├─────────────────────────────────────────────────────┤
//! │ >                                                 │
//! └─────────────────────────────────────────────────────┘
//! ```

use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::Text;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use vava_coding::CodingSession;
use vava_core::{AgentError, AgentEvent};

use crate::render::Renderer;

/// Maximum scrollback lines kept in memory.
const MAX_SCROLLBACK: usize = 10_000;

/// The state of one in-flight turn: the event stream to render, the channel
/// the session comes back on, and the cancellation token.
struct ActiveTurn {
    event_rx: mpsc::Receiver<AgentEvent>,
    session_rx: oneshot::Receiver<(CodingSession, Result<(), AgentError>)>,
    token: CancellationToken,
}

/// Restores the terminal if `run` exits on any path (including errors).
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let mut stdout = std::io::stdout();
        let _ = execute!(stdout, LeaveAlternateScreen);
    }
}

/// Run the TUI: an interactive session whose events are rendered into a
/// scrollable conversation panel.
pub async fn run(session: CodingSession, model: &str, show_reasoning: bool) -> anyhow::Result<()> {
    enable_raw_mode()?;
    let _guard = TerminalGuard; // restores the terminal on every exit path
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut state = TuiState::new(model, show_reasoning);
    let mut active: Option<ActiveTurn> = None;
    let mut session: Option<CodingSession> = Some(session);

    loop {
        terminal.draw(|frame| state.draw(frame))?;

        // Keyboard input (crossterm in raw mode; Ctrl-C arrives as a key).
        while event::poll(Duration::ZERO)? {
            match event::read()? {
                Event::Key(key) => {
                    if handle_key(&key, &mut state, &mut session, &mut active) {
                        return Ok(());
                    }
                }
                Event::Resize(_, _) => {}
                _ => {}
            }
        }

        // Model events from the active turn, rendered as they arrive.
        if let Some(turn) = &mut active {
            while let Ok(event) = turn.event_rx.try_recv() {
                state.render_event(&event);
            }
            if let Ok((session_back, turn_result)) = turn.session_rx.try_recv() {
                session = Some(session_back);
                active = None;
                state.finish_turn(turn_result);
            }
        }

        tokio::time::sleep(Duration::from_millis(16)).await;
    }
}

/// Handle one key event. Returns `true` when the TUI should exit.
fn handle_key(
    key: &KeyEvent,
    state: &mut TuiState,
    session: &mut Option<CodingSession>,
    active: &mut Option<ActiveTurn>,
) -> bool {
    match key.code {
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(turn) = active {
                // Ctrl-C while a turn runs cancels it.
                turn.token.cancel();
            } else {
                // Ctrl-C while idle exits.
                return true;
            }
        }
        KeyCode::Enter => {
            if active.is_some() {
                return false; // ignore input while busy
            }
            let prompt = state.input.trim().to_string();
            state.input.clear();
            if prompt.is_empty() {
                return false;
            }
            if matches!(prompt.as_str(), "quit" | "exit") {
                return true;
            }
            let Some(mut session) = session.take() else {
                return false;
            };
            let token = CancellationToken::new();
            let token_for_task = token.clone();
            let (event_tx, event_rx) = mpsc::channel(64);
            let (session_tx, session_rx) = oneshot::channel();
            tokio::spawn(async move {
                let result = session.prompt(prompt, event_tx, token_for_task).await;
                let _ = session_tx.send((session, result));
            });
            state.begin_turn();
            *active = Some(ActiveTurn {
                event_rx,
                session_rx,
                token,
            });
        }
        KeyCode::Char(c) => state.input.push(c),
        KeyCode::Backspace => {
            state.input.pop();
        }
        KeyCode::Up => state.scroll_up(),
        KeyCode::Down => state.scroll_down(),
        KeyCode::PageUp => state.scroll_page_up(),
        KeyCode::PageDown => state.scroll_page_down(),
        _ => {}
    }
    false
}

/// The TUI's view state.
struct TuiState {
    model: String,
    renderer: Renderer,
    /// Fully rendered, newline-terminated lines of the conversation.
    lines: Vec<String>,
    /// A partial line still being streamed (text deltas have no newline).
    pending: String,
    input: String,
    /// Whether to keep the view pinned to the newest lines.
    follow_bottom: bool,
    /// Scroll offset (in text lines) when not following the bottom.
    scroll_top: usize,
    /// Status message (e.g. "(cancelled)").
    status: String,
    /// Whether a turn is in flight.
    busy: bool,
}

impl TuiState {
    fn new(model: &str, show_reasoning: bool) -> Self {
        Self {
            model: model.to_string(),
            renderer: Renderer::new(show_reasoning),
            lines: Vec::new(),
            pending: String::new(),
            input: String::new(),
            follow_bottom: true,
            scroll_top: 0,
            status: String::new(),
            busy: false,
        }
    }

    fn begin_turn(&mut self) {
        self.busy = true;
        self.status.clear();
        self.pending.clear();
        self.follow_bottom = true;
    }

    fn finish_turn(&mut self, result: Result<(), AgentError>) {
        self.busy = false;
        // Flush any unterminated trailing line.
        if !self.pending.is_empty() {
            self.lines.push(std::mem::take(&mut self.pending));
        }
        self.status = match result {
            Ok(()) => String::new(),
            Err(AgentError::Cancelled) => "(cancelled)".into(),
            Err(error) => format!("error: {error}"),
        };
    }

    /// Render one agent event into the scrollback using the same renderer
    /// as print mode.
    fn render_event(&mut self, event: &AgentEvent) {
        let mut buf = Vec::new();
        if self.renderer.render(event, &mut buf).is_err() {
            return;
        }
        self.pending.push_str(&String::from_utf8_lossy(&buf));
        while let Some(end) = self.pending.find('\n') {
            let line = self.pending[..end].to_string();
            self.pending = self.pending[end + 1..].to_string();
            self.lines.push(line);
            if self.lines.len() > MAX_SCROLLBACK {
                self.lines.remove(0);
            }
        }
    }

    fn scroll_up(&mut self) {
        self.follow_bottom = false;
        self.scroll_top = self.scroll_top.saturating_sub(1);
    }

    fn scroll_down(&mut self) {
        self.scroll_top += 1;
    }

    fn scroll_page_up(&mut self) {
        self.follow_bottom = false;
        self.scroll_top = self.scroll_top.saturating_sub(10);
    }

    fn scroll_page_down(&mut self) {
        self.scroll_top += 10;
    }

    fn draw(&self, frame: &mut ratatui::Frame) {
        let chunks = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(3),
        ])
        .split(frame.area());

        // Title bar: name and model.
        let busy = if self.busy { " (working…)" } else { "" };
        let title = format!("vava{busy} — {}", self.model);
        frame.render_widget(Paragraph::new(title), chunks[0]);

        // Conversation panel.
        let height = chunks[1].height.saturating_sub(2) as usize; // borders
        let scroll = if self.follow_bottom {
            self.lines.len().saturating_sub(height)
        } else {
            self.scroll_top
        };
        let text = if self.lines.is_empty() {
            Text::raw("")
        } else {
            Text::raw(self.lines.join("\n"))
        };
        let status = if self.status.is_empty() {
            "conversation".to_string()
        } else {
            self.status.clone()
        };
        frame.render_widget(
            Paragraph::new(text)
                .block(Block::bordered().title(format!(" {status} ")))
                .wrap(Wrap { trim: false })
                .scroll((scroll as u16, 0)),
            chunks[1],
        );

        // Input line.
        let input = format!("> {}", self.input);
        let input_style = if self.busy {
            Style::default().add_modifier(Modifier::DIM)
        } else {
            Style::default()
        };
        frame.render_widget(
            Paragraph::new(input)
                .style(input_style)
                .block(Block::default().borders(Borders::ALL)),
            chunks[2],
        );
    }
}
