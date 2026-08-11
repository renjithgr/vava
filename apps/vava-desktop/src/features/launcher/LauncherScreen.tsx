import { useEffect } from "react";
import { StatusBanner } from "../../components/StatusBanner";
import { useVersion } from "../../hooks/useVersion";
import { relativeTime } from "../../lib/time";
import { useRepositoryStore } from "../../stores/repository";

/**
 * The startup screen: recent repositories plus the Open Folder action.
 *
 * D2 only — session management (D3) replaces the repository panel after
 * opening.
 */
export function LauncherScreen() {
  const {
    recents,
    loadingRecents,
    opening,
    error,
    openRepository,
    openPath,
    removeRecent,
    loadRecents,
    clearError,
  } = useRepositoryStore();
  const { version } = useVersion();

  useEffect(() => {
    void loadRecents();
  }, [loadRecents]);

  return (
    <main className="launcher">
      <header className="titlebar">
        <span className="brand">Vava</span>
      </header>

      <section className="launcher-panel">
        <div className="launcher-brand">
          <h1 className="launcher-title">Vava</h1>
          <p className="launcher-tagline">
            Coding agent for your repositories.
          </p>
        </div>

        {error && (
          <StatusBanner kind="error">
            <span className="banner-text">{error}</span>
            <button className="banner-close" onClick={clearError}>
              ✕
            </button>
          </StatusBanner>
        )}

        <h2 className="panel-heading">Recent repositories</h2>

        {recents.length === 0 && !loadingRecents && (
          <p className="empty">No recent repositories yet.</p>
        )}

        <ul className="recents">
          {recents.map((recent) => (
            <li key={recent.path} className="recent">
              <button
                className="recent-open"
                disabled={!recent.exists || opening}
                onClick={() => void openPath(recent.path)}
                title={recent.exists ? "Open repository" : "No longer exists"}
              >
                <span className="recent-name">{recent.name}</span>
                <span className="recent-path">{recent.path}</span>
              </button>
              <span className="recent-time">
                {relativeTime(recent.lastOpenedAt)}
              </span>
              {!recent.exists && (
                <button
                  className="recent-remove"
                  onClick={() => void removeRecent(recent.path)}
                  title="Remove from recents"
                >
                  ✕
                </button>
              )}
            </li>
          ))}
        </ul>

        <div className="launcher-actions">
          <button
            className="primary"
            onClick={() => void openRepository()}
            disabled={opening}
          >
            {opening ? "Opening…" : "Open Folder…"}
          </button>
        </div>

        {version && <p className="launcher-version">v{version}</p>}
      </section>
    </main>
  );
}
