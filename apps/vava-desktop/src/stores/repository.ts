import { create } from "zustand";
import { toMessage } from "../lib/errors";
import {
  ipc,
  type RecentRepository,
  type RepositoryInfo,
} from "../lib/ipc";

/**
 * Frontend repository state: the active repository and the recent list.
 *
 * Kept deliberately small (Phase 24). Session/turn state arrives with D3/D4;
 * streamed text will be batched in its own store so deltas never re-render
 * the whole application.
 */
interface RepositoryState {
  active: RepositoryInfo | null;
  recents: RecentRepository[];
  loadingRecents: boolean;
  opening: boolean;
  error: string | null;

  /** On app start: re-sync with the Rust state and load recents. */
  bootstrap: () => Promise<void>;
  loadRecents: () => Promise<void>;
  /** Open the native folder picker, then open the chosen repository. */
  openRepository: () => Promise<void>;
  /** Open a repository by path (e.g. from recents). */
  openPath: (path: string) => Promise<void>;
  removeRecent: (path: string) => Promise<void>;
  clearError: () => void;
}

/** Open a repository and refresh recents (no `opening` guard — callers own it). */
async function openPathIntoStore(
  set: (partial: Partial<RepositoryState>) => void,
  path: string,
) {
  const info = await ipc.openRepository(path);
  const recents = await ipc.listRecentRepositories();
  set({ active: info, recents });
}

export const useRepositoryStore = create<RepositoryState>()((set, get) => ({
  active: null,
  recents: [],
  loadingRecents: false,
  opening: false,
  error: null,

  bootstrap: async () => {
    try {
      const [active, recents] = await Promise.all([
        ipc.activeRepository(),
        ipc.listRecentRepositories(),
      ]);
      set({ active, recents });
    } catch (err) {
      set({ error: toMessage(err) });
    }
  },

  loadRecents: async () => {
    set({ loadingRecents: true });
    try {
      const recents = await ipc.listRecentRepositories();
      set({ recents, loadingRecents: false });
    } catch (err) {
      set({ error: toMessage(err), loadingRecents: false });
    }
  },

  openRepository: async () => {
    if (get().opening) return;
    set({ opening: true, error: null });
    try {
      const path = await ipc.pickFolder();
      if (path === null) {
        set({ opening: false });
        return;
      }
      await openPathIntoStore(set, path);
      set({ opening: false });
    } catch (err) {
      set({ error: toMessage(err), opening: false });
    }
  },

  openPath: async (path) => {
    if (get().opening) return;
    set({ opening: true, error: null });
    try {
      await openPathIntoStore(set, path);
      set({ opening: false });
    } catch (err) {
      set({ error: toMessage(err), opening: false });
    }
  },

  removeRecent: async (path) => {
    try {
      await ipc.removeRecentRepository(path);
      const recents = await ipc.listRecentRepositories();
      set({ recents });
    } catch (err) {
      set({ error: toMessage(err) });
    }
  },

  clearError: () => set({ error: null }),
}));
