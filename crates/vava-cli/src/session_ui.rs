//! Shared session presentation for the CLI: picker listings, banners, and
//! the `/session` info block. Both the REPL and the TUI render sessions
//! through these helpers so the output stays consistent.

use std::path::Path;

use chrono::{DateTime, Utc};

use vava_coding::{CodingSession, SessionSummary};

/// The outcome of interpreting one line of picker input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickResult<'a> {
    /// The line selected one session.
    Picked(&'a SessionSummary),
    /// The line was empty, out of range, or matched nothing.
    Cancelled,
    /// The line was a prefix matching several sessions.
    Ambiguous(Vec<&'a SessionSummary>),
}

/// Interpret a picker answer: a 1-based number or a session-id prefix.
/// A number within range picks by position (`0` cancels); any other input
/// (including numeric prefixes like `8472`) is matched as a session-id
/// prefix.
pub fn choose_session<'a>(input: &str, sessions: &'a [SessionSummary]) -> PickResult<'a> {
    let input = input.trim();
    if let Ok(number) = input.parse::<usize>() {
        match number {
            0 => return PickResult::Cancelled,
            n if n <= sessions.len() => return PickResult::Picked(&sessions[n - 1]),
            // Out of range: this may still be a numeric session-id prefix,
            // so fall through to prefix matching.
            _ => {}
        }
    }
    match vava_coding::resolve_prefix(sessions, input) {
        vava_coding::PrefixMatch::Unique(summary) => PickResult::Picked(summary),
        vava_coding::PrefixMatch::None => PickResult::Cancelled,
        vava_coding::PrefixMatch::Ambiguous(matches) => PickResult::Ambiguous(matches),
    }
}

/// The one-line banner shown when a session is resumed at startup:
/// `Resumed 01KABC — "Fix the failing payment tests"`.
pub fn resumed_banner(summary: &SessionSummary) -> String {
    match &summary.first_user_message {
        Some(first) => format!("Resumed {} — {:?}", summary.id.short(), truncate(first, 60)),
        None => format!("Resumed {}", summary.id.short()),
    }
}

/// The numbered picker listing shown by `vava -r` and `/resume`.
pub fn listing_lines(sessions: &[SessionSummary], root: &Path) -> Vec<String> {
    let mut lines = vec![format!("Sessions for {}", root.display()), String::new()];
    for (index, summary) in sessions.iter().enumerate() {
        let first = truncate(summary.first_user_message.as_deref().unwrap_or(""), 48);
        lines.push(format!(
            "{}. {}  {}  {:?}",
            index + 1,
            summary.id.short(),
            relative_time(summary.updated_at),
            first
        ));
    }
    lines
}

/// The `/session` info block.
pub fn info_lines(session: &CodingSession) -> Vec<String> {
    let summary = session.summary();
    vec![
        format!("Session:    {}", summary.id.full()),
        format!("Repository: {}", summary.repository_root.display()),
        format!(
            "Created:    {}",
            summary.created_at.format("%Y-%m-%d %H:%M")
        ),
        format!(
            "Updated:    {}",
            summary.updated_at.format("%Y-%m-%d %H:%M")
        ),
        format!("Messages:   {}", session.messages().len()),
    ]
}

/// A compact relative timestamp: `12 min ago`, `yesterday`, `3 days ago`.
pub fn relative_time(updated: DateTime<Utc>) -> String {
    let delta = Utc::now().signed_duration_since(updated);
    if delta.num_seconds() < 60 {
        "just now".to_string()
    } else if delta.num_minutes() < 60 {
        format!("{} min ago", delta.num_minutes())
    } else if delta.num_hours() < 24 {
        format!("{} hr ago", delta.num_hours())
    } else if delta.num_days() < 2 {
        "yesterday".to_string()
    } else {
        format!("{} days ago", delta.num_days())
    }
}

/// Truncate `text` to at most `max` characters, adding an ellipsis when
/// something was cut. Always returns a valid UTF-8 string.
pub fn truncate(text: &str, max: usize) -> String {
    let mut chars = text.chars();
    let mut out: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use chrono::{TimeZone, Utc};
    use vava_coding::SessionId;

    fn summary(id: &str, minutes_ago: i64, first: &str) -> SessionSummary {
        SessionSummary {
            id: SessionId::new(id),
            repository_root: PathBuf::from("/repo"),
            created_at: Utc::now(),
            updated_at: Utc::now() - chrono::Duration::minutes(minutes_ago),
            first_user_message: Some(first.into()),
        }
    }

    #[test]
    fn picker_accepts_numbers() {
        let sessions = vec![
            summary("01KABC", 1, "fix tests"),
            summary("01K9ZZ", 2, "refactor"),
        ];
        assert_eq!(
            choose_session("1", &sessions),
            PickResult::Picked(&sessions[0])
        );
        assert_eq!(
            choose_session("2", &sessions),
            PickResult::Picked(&sessions[1])
        );
        assert_eq!(choose_session("0", &sessions), PickResult::Cancelled);
        assert_eq!(choose_session("3", &sessions), PickResult::Cancelled);
    }

    #[test]
    fn picker_accepts_unique_prefixes_and_reports_ambiguity() {
        let sessions = vec![summary("01KABC111", 1, "a"), summary("01KABC222", 2, "b")];
        assert_eq!(
            choose_session("01KABC111", &sessions),
            PickResult::Picked(&sessions[0])
        );
        match choose_session("01KABC", &sessions) {
            PickResult::Ambiguous(matches) => assert_eq!(matches.len(), 2),
            other => panic!("expected ambiguity, got {other:?}"),
        }
        assert_eq!(choose_session("zzz", &sessions), PickResult::Cancelled);
        assert_eq!(choose_session("", &sessions), PickResult::Cancelled);
    }

    #[test]
    fn numeric_prefixes_fall_through_to_prefix_matching() {
        // An out-of-range number is a numeric session-id prefix, not an
        // index: "8472" must pick 847237fa… rather than cancel.
        let sessions = vec![
            summary("847237fa-8b2c", 1, "a"),
            summary("01KABC111", 2, "b"),
        ];
        assert_eq!(
            choose_session("8472", &sessions),
            PickResult::Picked(&sessions[0])
        );
        // An out-of-range number that matches nothing cancels.
        assert_eq!(choose_session("99999", &sessions), PickResult::Cancelled);
    }

    #[test]
    fn relative_time_renders_compact_units() {
        assert_eq!(relative_time(Utc::now()), "just now");
        assert_eq!(
            relative_time(Utc::now() - chrono::Duration::minutes(12)),
            "12 min ago"
        );
        assert_eq!(
            relative_time(Utc::now() - chrono::Duration::hours(3)),
            "3 hr ago"
        );
        assert_eq!(
            relative_time(Utc::now() - chrono::Duration::days(1)),
            "yesterday"
        );
        assert_eq!(
            relative_time(Utc::now() - chrono::Duration::days(3)),
            "3 days ago"
        );
    }

    #[test]
    fn truncate_adds_an_ellipsis() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("a very long message", 10), "a very lon…");
    }

    #[test]
    fn resumed_banner_shows_the_first_prompt() {
        let s = summary("01KABC", 1, "Fix the failing payment tests");
        assert_eq!(
            resumed_banner(&s),
            "Resumed 01KABC — \"Fix the failing payment tests\""
        );
    }

    #[test]
    fn listing_lines_are_numbered_and_quoted() {
        let sessions = vec![summary("01KABC", 5, "fix tests")];
        let lines = listing_lines(&sessions, Path::new("/repo"));
        assert_eq!(lines[0], "Sessions for /repo");
        assert!(lines[2].starts_with("1. 01KABC"));
        assert!(lines[2].contains("\"fix tests\""));
        assert!(lines[2].contains("5 min ago"));
    }

    #[test]
    fn timestamps_format_as_local_dates() {
        let dt = Utc.with_ymd_and_hms(2026, 8, 11, 14, 23, 0).unwrap();
        assert_eq!(dt.format("%Y-%m-%d %H:%M").to_string(), "2026-08-11 14:23");
    }
}
