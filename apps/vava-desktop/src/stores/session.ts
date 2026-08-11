import { Channel } from "@tauri-apps/api/core";
import { create } from "zustand";
import { toMessage } from "../lib/errors";
import {
  ipc,
  type DesktopAgentEvent,
  type DesktopMessage,
  type SessionInfo,
  type SessionView,
} from "../lib/ipc";
import { useRepositoryStore } from "./repository";

/** A tool call in progress during a live turn. */
interface StreamingToolCall {
  callId: string;
  tool: string;
  input: unknown;
  result: { content: string; isError: boolean } | null;
}

/** The partial assistant message being streamed. */
interface StreamingAssistant {
  content: string;
  reasoning: string;
  toolCalls: StreamingToolCall[];
}

function emptyStreaming(): StreamingAssistant {
  return { content: "", reasoning: "", toolCalls: [] };
}

/**
 * Session state: the sidebar list, the active session's transcript, and
 * live turn streaming (D4).
 *
 * Streaming is buffered in a dedicated `streaming` slice so text deltas
 * only re-render the streaming bubble, never the whole conversation.
 * `epoch` guards against events from a stale (cancelled/switched) turn.
 */
interface SessionState {
  sessions: SessionInfo[];
  activeSession: SessionView | null;
  loading: boolean;
  error: string | null;
  /** Whether a turn is currently running. */
  running: boolean;
  /** The live partial assistant message, while streaming. */
  streaming: StreamingAssistant | null;
  /** Results of finished tool calls, keyed by call id. */
  toolResults: Record<string, { content: string; isError: boolean }>;
  /** Bumped on every send/switch; events from older turns are ignored. */
  epoch: number;

  /** List sessions and select the repository's active session (if any). */
  load: () => Promise<void>;
  /** Switch to a persisted session and restore its transcript. */
  select: (id: string) => Promise<void>;
  /** Start a brand-new session (equivalent to the terminal `/new`). */
  create: () => Promise<void>;
  /** Send a prompt and stream the turn into the conversation. */
  send: (input: string) => Promise<void>;
  /** Cancel the running turn. */
  stop: () => Promise<void>;
  /** Apply one streamed turn event (internal; channel callback). */
  applyEvent: (epoch: number, event: DesktopAgentEvent) => void;
  clearError: () => void;
}

export const useSessionStore = create<SessionState>()((set, get) => ({
  sessions: [],
  activeSession: null,
  loading: false,
  error: null,
  running: false,
  streaming: null,
  toolResults: {},
  epoch: 0,

  load: async () => {
    const repo = useRepositoryStore.getState().active;
    if (!repo) return;
    set({ loading: true, error: null });
    try {
      const sessions = await ipc.listSessions();
      set({ sessions });
      const currentId = get().activeSession?.session.id ?? repo.activeSessionId;
      if (currentId) {
        await get().select(currentId);
      } else {
        set({ activeSession: null });
      }
      set({ loading: false });
    } catch (err) {
      set({ error: toMessage(err), loading: false });
    }
  },

  select: async (id) => {
    set({
      loading: true,
      error: null,
      running: false,
      streaming: null,
      epoch: get().epoch + 1,
    });
    try {
      const view = await ipc.selectSession(id);
      set({ activeSession: view, loading: false });
    } catch (err) {
      set({ error: toMessage(err), loading: false });
    }
  },

  create: async () => {
    set({
      loading: true,
      error: null,
      running: false,
      streaming: null,
      epoch: get().epoch + 1,
    });
    try {
      const view = await ipc.newSession();
      const sessions = await ipc.listSessions();
      set({ activeSession: view, sessions, loading: false });
    } catch (err) {
      set({ error: toMessage(err), loading: false });
    }
  },

  send: async (input) => {
    const session = get().activeSession;
    if (!session) {
      set({ error: "No active session." });
      return;
    }
    if (get().running) return;

    const epoch = get().epoch + 1;
    set({
      epoch,
      running: true,
      error: null,
      streaming: emptyStreaming(),
      toolResults: {},
      // Optimistically append the user message; the harness records it in
      // the transcript before the first event arrives.
      activeSession: {
        ...session,
        messages: [...session.messages, userMessage(input)],
      },
    });

    const channel = new Channel<DesktopAgentEvent>();
    channel.onmessage = (event) => get().applyEvent(epoch, event);
    try {
      await ipc.sendPrompt(session.session.id, input, channel);
    } catch (err) {
      set({ error: toMessage(err), running: false });
    }
  },

  stop: async () => {
    try {
      await ipc.cancelTurn();
    } catch (err) {
      set({ error: toMessage(err) });
    }
  },

  clearError: () => set({ error: null }),

  applyEvent: (epoch, event: DesktopAgentEvent) => {
    if (get().epoch !== epoch) return; // stale turn
    const state = get();
    switch (event.type) {
      case "turn_started":
        break;
      case "text_delta": {
        const s = state.streaming ?? emptyStreaming();
        set({ streaming: { ...s, content: s.content + event.delta } });
        break;
      }
      case "reasoning_delta": {
        const s = state.streaming ?? emptyStreaming();
        set({ streaming: { ...s, reasoning: s.reasoning + event.delta } });
        break;
      }
      case "tool_call_started": {
        const s = state.streaming ?? emptyStreaming();
        set({
          streaming: {
            ...s,
            toolCalls: [
              ...s.toolCalls,
              { callId: event.callId, tool: event.tool, input: event.input, result: null },
            ],
          },
        });
        break;
      }
      case "tool_call_finished": {
        const s = state.streaming;
        set({
          streaming: s
            ? {
                ...s,
                toolCalls: s.toolCalls.map((call) =>
                  call.callId === event.callId
                    ? { ...call, result: event.result }
                    : call,
                ),
              }
            : s,
          toolResults: { ...state.toolResults, [event.callId]: event.result },
        });
        break;
      }
      case "assistant_message_completed": {
        const current = get().activeSession;
        if (!current) break;
        set({
          activeSession: {
            ...current,
            messages: [...current.messages, event.message],
          },
          streaming: null,
        });
        break;
      }
      case "turn_completed": {
        set({ running: false, streaming: null });
        // Refresh the sidebar (timestamps/first prompt) and reconcile the
        // transcript with the authoritative persisted version.
        void get().load();
        break;
      }
      case "error": {
        const cancelled = event.message === "operation cancelled";
        set({
          running: false,
          streaming: null,
          error: cancelled ? null : event.message,
        });
        break;
      }
    }
  },
}));

function userMessage(content: string): DesktopMessage {
  return { type: "user", content };
}
