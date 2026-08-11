import { relativeTime } from "../lib/time";
import { useSessionStore } from "../stores/session";

/**
 * The session sidebar: newest-first list, active indicator, and a
 * "+ New Session" action (Phase 7/8).
 *
 * Clicking a session cancels any running turn (D4+), loads it, and appends
 * future messages to it — never duplicates it. The list is (re)loaded by
 * `RepositoryScreen` whenever the active repository changes.
 */
export function SessionSidebar() {
  const { sessions, activeSession, loading, error, select, create, clearError } =
    useSessionStore();

  const activeId = activeSession?.session.id ?? null;

  return (
    <aside className="sidebar">
      <div className="sidebar-header">
        <span className="panel-heading">Sessions</span>
        <button
          className="ghost small"
          onClick={() => void create()}
          disabled={loading}
          title="Start a fresh session"
        >
          + New Session
        </button>
      </div>

      {error && (
        <div className="sidebar-error">
          <span className="sidebar-error-text">{error}</span>
          <button className="banner-close" onClick={clearError}>
            ✕
          </button>
        </div>
      )}

      {sessions.length === 0 && !loading && (
        <p className="empty sidebar-empty">No sessions yet.</p>
      )}

      <ul className="sidebar-sessions">
        {sessions.map((session) => (
          <li key={session.id}>
            <button
              className={session.id === activeId ? "session-row active" : "session-row"}
              onClick={() => void select(session.id)}
              title="Resume this session"
            >
              <span className="session-row-title">
                {session.firstUserMessage ?? "(empty session)"}
              </span>
              <span className="session-row-time">
                {relativeTime(session.updatedAt)}
              </span>
            </button>
          </li>
        ))}
      </ul>
    </aside>
  );
}
