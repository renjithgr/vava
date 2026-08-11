import { create } from "zustand";
import { toMessage } from "../lib/errors";
import { ipc, type SessionInfo, type SessionView } from "../lib/ipc";
import { useRepositoryStore } from "./repository";

/**
 * Session state: the sidebar list plus the active session's transcript.
 *
 * D3 renders the persisted transcript read-only; D4 adds streaming into
 * the active session (buffered so text deltas never re-render everything).
 */
interface SessionState {
  sessions: SessionInfo[];
  activeSession: SessionView | null;
  loading: boolean;
  error: string | null;

  /** List sessions and select the repository's active session (if any). */
  load: () => Promise<void>;
  /** Switch to a persisted session and restore its transcript. */
  select: (id: string) => Promise<void>;
  /** Start a brand-new session (equivalent to the terminal `/new`). */
  create: () => Promise<void>;
  clearError: () => void;
}

export const useSessionStore = create<SessionState>()((set, get) => ({
  sessions: [],
  activeSession: null,
  loading: false,
  error: null,

  load: async () => {
    const repo = useRepositoryStore.getState().active;
    if (!repo) return;
    set({ loading: true, error: null });
    try {
      const sessions = await ipc.listSessions();
      set({ sessions });
      if (repo.activeSessionId) {
        await get().select(repo.activeSessionId);
      } else {
        set({ activeSession: null });
      }
      set({ loading: false });
    } catch (err) {
      set({ error: toMessage(err), loading: false });
    }
  },

  select: async (id) => {
    set({ loading: true, error: null });
    try {
      const view = await ipc.selectSession(id);
      set({ activeSession: view, loading: false });
    } catch (err) {
      set({ error: toMessage(err), loading: false });
    }
  },

  create: async () => {
    set({ loading: true, error: null });
    try {
      const view = await ipc.newSession();
      const sessions = await ipc.listSessions();
      set({ activeSession: view, sessions, loading: false });
    } catch (err) {
      set({ error: toMessage(err), loading: false });
    }
  },

  clearError: () => set({ error: null }),
}));
