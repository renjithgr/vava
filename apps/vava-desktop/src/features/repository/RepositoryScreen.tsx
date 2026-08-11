import { StatusBanner } from "../../components/StatusBanner";
import { relativeTime } from "../../lib/time";
import { useRepositoryStore } from "../../stores/repository";

/**
 * The repository view after opening a folder: name, root, and its saved
 * sessions. D2 renders sessions read-only; D3 makes them selectable and
 * adds the session sidebar, and D4 adds the conversation.
 */
export function RepositoryScreen() {
  const { active, opening, error, openRepository, clearError } =
    useRepositoryStore();

  if (!active) return null;

  const count = active.sessions.length;

  return (
    <main className="repo">
      <header className="titlebar">
        <span className="brand">Vava</span>
        <span className="repo-name">{active.name}</span>
        <span className="spacer" />
        <button
          className="ghost"
          onClick={() => void openRepository()}
          disabled={opening}
        >
          Open Folder…
        </button>
      </header>

      {error && (
        <StatusBanner kind="error">
          <span className="banner-text">{error}</span>
          <button className="banner-close" onClick={clearError}>
            ✕
          </button>
        </StatusBanner>
      )}

      <section className="repo-body">
        <div className="repo-header">
          <h2 className="repo-title">{active.name}</h2>
          <p className="repo-root">{active.root}</p>
          <p className="repo-meta">
            {count} session{count === 1 ? "" : "s"}
            {active.activeSessionId ? " · active session ready" : ""}
          </p>
        </div>

        <h3 className="panel-heading">Sessions</h3>
        {count === 0 && <p className="empty">No sessions yet.</p>}
        <ul className="sessions">
          {active.sessions.map((session) => (
            <li
              key={session.id}
              className={
                session.id === active.activeSessionId
                  ? "session session-active"
                  : "session"
              }
            >
              <span className="session-title">
                {session.firstUserMessage ?? "(empty session)"}
              </span>
              <span className="session-time">
                {relativeTime(session.updatedAt)}
              </span>
            </li>
          ))}
        </ul>
        <p className="note">Session management arrives in the next milestone.</p>
      </section>
    </main>
  );
}
