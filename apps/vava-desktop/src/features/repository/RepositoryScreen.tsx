import { useEffect } from "react";
import { PromptEditor } from "../../components/PromptEditor";
import { SessionSidebar } from "../../components/SessionSidebar";
import { StatusBanner } from "../../components/StatusBanner";
import { useRepositoryStore } from "../../stores/repository";
import { useSessionStore } from "../../stores/session";
import { ConversationView } from "../conversation/ConversationView";

/**
 * The main workspace layout (Phase 6): titlebar, session sidebar, the
 * conversation panel, and the prompt bar (D4).
 */
export function RepositoryScreen() {
  const { active, opening, error, openRepository, clearError } =
    useRepositoryStore();
  const running = useSessionStore((state) => state.running);

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
        {running && <span className="working">working…</span>}
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
        <div className="main-column">
          <ConversationView />
          <PromptEditor />
        </div>
      </div>
    </main>
  );
}
