/**
 * The single IPC boundary for the frontend.
 *
 * All Rust communication goes through the typed wrappers here — components
 * never call `invoke` directly. The wrapper set grows with each milestone,
 * but the boundary stays in one place so the Tauri surface is easy to audit.
 */
import { Channel, invoke } from "@tauri-apps/api/core";
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

/** One tool call within an assistant message. */
export interface DesktopToolCall {
  id: string;
  name: string;
  arguments: unknown;
}

/**
 * One transcript message. The wire shape mirrors vava's JSONL persistence
 * format (tagged `type`, snake_case fields) so the original textual
 * session data is preserved exactly.
 */
export type DesktopMessage =
  | { type: "user"; content: string }
  | {
      type: "assistant";
      content: string;
      reasoning_content?: string | null;
      tool_calls?: DesktopToolCall[];
    }
  | {
      type: "tool";
      tool_call_id: string;
      tool_name: string;
      content: string;
      is_error: boolean;
    };

/** A loaded session: its summary plus the restored transcript. */
export interface SessionView {
  session: SessionInfo;
  messages: DesktopMessage[];
}

/**
 * One event streamed from the agent harness to React during a turn.
 * Payload fields are camelCase; transcript messages embedded in
 * `assistant_message_completed` keep the snake_case persistence shape.
 */
export type DesktopAgentEvent =
  | { type: "turn_started" }
  | { type: "text_delta"; delta: string }
  | { type: "reasoning_delta"; delta: string }
  | {
      type: "tool_call_started";
      callId: string;
      tool: string;
      input: unknown;
    }
  | {
      type: "tool_call_finished";
      callId: string;
      result: { content: string; isError: boolean };
    }
  | { type: "assistant_message_completed"; message: DesktopMessage }
  | { type: "turn_completed" }
  | { type: "error"; message: string };

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

  /** The active repository's sessions, newest first. */
  listSessions: () => invoke<SessionInfo[]>("list_sessions"),

  /** Switch to a persisted session and return its restored transcript. */
  selectSession: (sessionId: string) =>
    invoke<SessionView>("select_session", { sessionId }),

  /** Start a brand-new session and return it (empty transcript). */
  newSession: () => invoke<SessionView>("new_session"),

  /**
   * Run one prompt on the active session, streaming agent events to the
   * channel. Resolves once the turn has started.
   */
  sendPrompt: (
    sessionId: string,
    input: string,
    channel: Channel<DesktopAgentEvent>,
  ) => invoke<void>("send_prompt", { sessionId, input, channel }),

  /** Cancel the running turn (the Stop button). */
  cancelTurn: () => invoke<void>("cancel_turn"),
};
