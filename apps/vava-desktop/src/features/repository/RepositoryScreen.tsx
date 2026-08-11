import { StatusBanner } from "../../components/StatusBanner";
import { SessionSidebar } from "../../components/SessionSidebar";
import { useRepositoryStore } from "../../stores/repository";
import { useSessionStore } from "../../stores/session";
import { ConversationView } from "../conversation/ConversationView";
import { useEffect } from "react";

/**
 * The main workspace layout (Phase 6): titlebar, session sidebar, and the
 * conversation panel. D3 fills the conversation with the persisted
 * transcript; the prompt bar arrives with D4.
 */
export function RepositoryScreen() {
  const { active, opening, error, openRepository, clearError } =
    useRepositoryStore();

  // Reload the session sidebar whenever the active repository changes
  // (including re-opening the same folder, which starts a fresh session).
  useEffect(() => {
    if (active) void useSessionStore.getState().load();
  }, [active]);

  if (!active) return null;

  return (
    <main className="app">
      <header className="titlebar">
        <span className="brand">Vava</span>
        <span className="repo-name" title={active.root}>
          {active.name}
        </span>
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

      <div className="app-body">
        <SessionSidebar />
        <ConversationView />
      </div>
    </main>
  );
}
