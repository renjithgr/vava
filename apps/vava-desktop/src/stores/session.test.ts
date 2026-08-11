import { beforeEach, describe, expect, it, vi } from "vitest";

// The stores use the Tauri runtime and IPC; both are mocked so the
// prompt-lifecycle and event-reducer logic is testable without a webview.
vi.mock("@tauri-apps/api/core", () => {
  class MockChannel {
    onmessage: ((event: unknown) => void) | null = null;
  }
  return { Channel: MockChannel, invoke: vi.fn() };
});

vi.mock("../lib/ipc", () => ({
  ipc: {
    getVersion: vi.fn(),
    pickFolder: vi.fn(),
    openRepository: vi.fn(),
    activeRepository: vi.fn(),
    listRecentRepositories: vi.fn(),
    removeRecentRepository: vi.fn(),
    listSessions: vi.fn(),
    selectSession: vi.fn(),
    newSession: vi.fn(),
    sendPrompt: vi.fn(),
    cancelTurn: vi.fn(),
  },
}));

import type { DesktopAgentEvent, SessionView } from "../lib/ipc";
import { ipc } from "../lib/ipc";
import { useSessionStore } from "./session";

const sendPromptMock = vi.mocked(ipc.sendPrompt);

const view: SessionView = {
  session: { id: "s1", createdAt: "2026-08-11T12:00:00Z", updatedAt: "2026-08-11T12:00:00Z", firstUserMessage: null },
  messages: [],
};

function seedActiveSession() {
  useSessionStore.setState({ activeSession: view });
}

function apply(event: DesktopAgentEvent, epoch = 0) {
  useSessionStore.getState().applyEvent(epoch, event);
}

beforeEach(() => {
  useSessionStore.setState({
    sessions: [],
    activeSession: null,
    loading: false,
    error: null,
    running: false,
    streaming: null,
    toolResults: {},
    epoch: 0,
  });
  sendPromptMock.mockReset();
});

describe("event reducer (applyEvent)", () => {
  it("accumulates text deltas into the streaming message", () => {
    seedActiveSession();
    apply({ type: "text_delta", delta: "Hello " });
    apply({ type: "text_delta", delta: "world" });
    expect(useSessionStore.getState().streaming?.content).toBe("Hello world");
  });

  it("accumulates reasoning separately from content", () => {
    seedActiveSession();
    apply({ type: "reasoning_delta", delta: "think " });
    apply({ type: "text_delta", delta: "answer" });
    const streaming = useSessionStore.getState().streaming!;
    expect(streaming.reasoning).toBe("think ");
    expect(streaming.content).toBe("answer");
  });

  it("tracks tool calls with live results", () => {
    seedActiveSession();
    apply({ type: "tool_call_started", callId: "c1", tool: "read", input: { path: "x" } });
    apply({
      type: "tool_call_finished",
      callId: "c1",
      result: { content: "file contents", isError: false },
    });
    const streaming = useSessionStore.getState().streaming!;
    expect(streaming.toolCalls[0].tool).toBe("read");
    expect(streaming.toolCalls[0].result?.content).toBe("file contents");
    expect(useSessionStore.getState().toolResults["c1"]).toEqual({
      content: "file contents",
      isError: false,
    });
  });

  it("appends completed assistant messages and clears the streaming bubble", () => {
    seedActiveSession();
    apply({ type: "text_delta", delta: "partial" });
    apply({
      type: "assistant_message_completed",
      message: { type: "assistant", content: "final" },
    });
    const state = useSessionStore.getState();
    expect(state.activeSession?.messages).toEqual([
      { type: "assistant", content: "final" },
    ]);
    expect(state.streaming).toBeNull();
  });

  it("turn_completed ends the running state", () => {
    useSessionStore.setState({ running: true, streaming: { content: "x", reasoning: "", toolCalls: [] } });
    apply({ type: "turn_completed" });
    const state = useSessionStore.getState();
    expect(state.running).toBe(false);
    expect(state.streaming).toBeNull();
  });

  it("an error event ends the turn and surfaces the message", () => {
    useSessionStore.setState({ running: true });
    apply({ type: "error", message: "the DeepSeek request failed" });
    const state = useSessionStore.getState();
    expect(state.running).toBe(false);
    expect(state.error).toBe("the DeepSeek request failed");
  });

  it("cancellation does not surface as an error banner", () => {
    useSessionStore.setState({ running: true });
    apply({ type: "error", message: "operation cancelled" });
    const state = useSessionStore.getState();
    expect(state.running).toBe(false);
    expect(state.error).toBeNull();
  });

  it("ignores events from a stale turn (epoch mismatch)", () => {
    seedActiveSession();
    apply({ type: "text_delta", delta: "from old turn" }, 3);
    expect(useSessionStore.getState().streaming).toBeNull();
  });
});

describe("prompt lifecycle (send/stop)", () => {
  it("send appends the user message, marks running, and streams through the channel", async () => {
    seedActiveSession();
    sendPromptMock.mockResolvedValue(undefined);

    await useSessionStore.getState().send("hello");

    const state = useSessionStore.getState();
    expect(state.running).toBe(true);
    expect(state.activeSession?.messages).toEqual([
      { type: "user", content: "hello" },
    ]);
    expect(sendPromptMock).toHaveBeenCalledWith(
      "s1",
      "hello",
      expect.objectContaining({ onmessage: expect.any(Function) }),
    );

    // Stream a delta through the channel captured by the invoke.
    const channel = sendPromptMock.mock.calls[0][2];
    (channel as { onmessage: ((e: unknown) => void) | null }).onmessage?.({
      type: "text_delta",
      delta: "hi",
    });
    expect(useSessionStore.getState().streaming?.content).toBe("hi");
  });

  it("send does nothing while a turn is already running", async () => {
    seedActiveSession();
    useSessionStore.setState({ running: true });
    await useSessionStore.getState().send("second");
    expect(sendPromptMock).not.toHaveBeenCalled();
  });

  it("send without an active session reports an error", async () => {
    await useSessionStore.getState().send("orphan");
    expect(useSessionStore.getState().error).toBe("No active session.");
    expect(sendPromptMock).not.toHaveBeenCalled();
  });

  it("stop cancels the running turn", async () => {
    sendPromptMock.mockResolvedValue(undefined);
    seedActiveSession();
    await useSessionStore.getState().send("hello");
    sendPromptMock.mockClear();

    (vi.mocked(ipc.cancelTurn) as ReturnType<typeof vi.fn>).mockResolvedValue(undefined);
    await useSessionStore.getState().stop();
    expect(ipc.cancelTurn).toHaveBeenCalled();
  });

  it("send failure surfaces the error and clears running", async () => {
    seedActiveSession();
    sendPromptMock.mockRejectedValue("DeepSeek API key is not configured.");
    await useSessionStore.getState().send("hello");
    const state = useSessionStore.getState();
    expect(state.running).toBe(false);
    expect(state.error).toBe("DeepSeek API key is not configured.");
  });
});
