import { describe, expect, it } from "vitest";
import type { DesktopMessage } from "../../lib/ipc";
import { normalizeMessages } from "./normalize";

const messages: DesktopMessage[] = [
  { type: "user", content: "fix the tests" },
  {
    type: "assistant",
    content: "",
    reasoning_content: "thinking",
    tool_calls: [
      { id: "c1", name: "bash", arguments: { command: "cargo test" } },
      { id: "c2", name: "read", arguments: { path: "src/lib.rs" } },
    ],
  },
  {
    type: "tool",
    tool_call_id: "c1",
    tool_name: "bash",
    content: "exit code: 101\nduration: 2.40s\n",
    is_error: true,
  },
  {
    type: "tool",
    tool_call_id: "c2",
    tool_name: "read",
    content: "1 | fn main() {}",
    is_error: false,
  },
  { type: "assistant", content: "the issue was…" },
];

describe("normalizeMessages", () => {
  it("merges tool calls with their transcript results into one card each", () => {
    const items = normalizeMessages(messages, {});
    expect(items).toHaveLength(5);
    expect(items[0]).toEqual({ kind: "user", content: "fix the tests" });
    // Assistant with reasoning but no content still renders.
    expect(items[1]).toEqual({ kind: "assistant", content: "", reasoning: "thinking" });
    // Two tool cards, each with the matching result attached.
    expect(items[2]).toMatchObject({
      kind: "tool",
      tool: "bash",
      args: { command: "cargo test" },
      result: { content: "exit code: 101\nduration: 2.40s\n", isError: true },
      running: false,
    });
    expect(items[3]).toMatchObject({
      kind: "tool",
      tool: "read",
      result: { content: "1 | fn main() {}", isError: false },
      running: false,
    });
    expect(items[4]).toEqual({ kind: "assistant", content: "the issue was…", reasoning: null });
  });

  it("uses live streamed results when no transcript tool message follows", () => {
    const items = normalizeMessages(
      [
        {
          type: "assistant",
          content: "",
          tool_calls: [{ id: "c1", name: "bash", arguments: { command: "ls" } }],
        },
      ],
      { c1: { content: "live result", isError: false } },
    );
    expect(items[0]).toMatchObject({
      kind: "tool",
      tool: "bash",
      result: { content: "live result", isError: false },
      running: false,
    });
  });

  it("marks tool calls without results as running (live turn)", () => {
    const items = normalizeMessages(
      [
        {
          type: "assistant",
          content: "",
          tool_calls: [{ id: "c1", name: "edit", arguments: { path: "x" } }],
        },
      ],
      {},
    );
    expect(items[0]).toMatchObject({ kind: "tool", tool: "edit", running: true, result: null });
  });

  it("renders standalone tool messages as cards without arguments", () => {
    const items = normalizeMessages(
      [
        {
          type: "tool",
          tool_call_id: "orphan",
          tool_name: "bash",
          content: "out",
          is_error: false,
        },
      ],
      {},
    );
    expect(items[0]).toEqual({
      kind: "tool",
      tool: "bash",
      args: null,
      result: { content: "out", isError: false },
      running: false,
    });
  });
});
