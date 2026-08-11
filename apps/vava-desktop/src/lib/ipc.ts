/**
 * The single IPC boundary for the frontend.
 *
 * All Rust communication goes through the typed wrappers here — components
 * never call `invoke` directly. The wrapper set grows with each milestone,
 * but the boundary stays in one place so the Tauri surface is easy to audit.
 */
import { invoke } from "@tauri-apps/api/core";
import { open as openFolderDialog } from "@tauri-apps/plugin-dialog";

/** One saved session, as shown in the session sidebar. */
export interface SessionInfo {
  id: string;
  createdAt: string;
  updatedAt: string;
  firstUserMessage: string | null;
}

/** The information returned after opening a repository. */
export interface RepositoryInfo {
  id: string;
  name: string;
  root: string;
  activeSessionId: string | null;
  sessions: SessionInfo[];
}

/** One recent repository, as shown on the launcher screen. */
export interface RecentRepository {
  path: string;
  name: string;
  lastOpenedAt: string;
  exists: boolean;
}

export const ipc = {
  /** The desktop app version, proving the Rust ↔ React round trip. */
  getVersion: () => invoke<string>("get_version"),

  /** Opens the native folder picker; returns the chosen path or null. */
  pickFolder: async (): Promise<string | null> => {
    const picked = await openFolderDialog({
      directory: true,
      multiple: false,
      title: "Open Folder…",
    });
    return typeof picked === "string" ? picked : null;
  },

  /** Open a repository at `path`: resolves the root, inits a session. */
  openRepository: (path: string) =>
    invoke<RepositoryInfo>("open_repository", { path }),

  /** The currently open repository, if any. */
  activeRepository: () => invoke<RepositoryInfo | null>("active_repository"),

  /** Recent repositories, newest first. */
  listRecentRepositories: () =>
    invoke<RecentRepository[]>("list_recent_repositories"),

  /** Remove a repository from recents. */
  removeRecentRepository: (path: string) =>
    invoke<void>("remove_recent_repository", { path }),
};
