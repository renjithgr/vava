import type { DesktopMessage } from "../../lib/ipc";
import type { ToolResultInfo } from "./ToolCard";

/** A normalized render item for the conversation panel. */
export type RenderItem =
  | { kind: "user"; content: string }
  | { kind: "assistant"; content: string; reasoning: string | null }
  | {
      kind: "tool";
      tool: string;
      args: unknown;
      result: ToolResultInfo | null;
      running: boolean;
    };

/**
 * Normalize the persisted transcript for rendering: assistant tool calls
 * become cards whose results come from the live `toolResults` map (streamed
 * turns) or from the matching transcript tool message (persisted sessions).
 * Standalone tool messages (older logs) become cards without arguments.
 */
export function normalizeMessages(
  messages: DesktopMessage[],
  toolResults: Record<string, ToolResultInfo>,
): RenderItem[] {
  const items: RenderItem[] = [];
  const pending = new Map<string, number>();

  for (const message of messages) {
    switch (message.type) {
      case "user":
        items.push({ kind: "user", content: message.content });
        break;
      case "assistant": {
        if (message.content !== "" || message.reasoning_content) {
          items.push({
            kind: "assistant",
            content: message.content,
            reasoning: message.reasoning_content ?? null,
          });
        }
        for (const call of message.tool_calls ?? []) {
          const live = toolResults[call.id];
          const index = items.length;
          items.push({
            kind: "tool",
            tool: call.name,
            args: call.arguments,
            result: live ?? null,
            running: live === undefined,
          });
          pending.set(call.id, index);
        }
        break;
      }
      case "tool": {
        const index = pending.get(message.tool_call_id);
        if (index !== undefined) {
          const item = items[index];
          if (item.kind === "tool") {
            item.result = { content: message.content, isError: message.is_error };
            item.running = false;
          }
          pending.delete(message.tool_call_id);
        } else {
          items.push({
            kind: "tool",
            tool: message.tool_name,
            args: null,
            result: { content: message.content, isError: message.is_error },
            running: false,
          });
        }
        break;
      }
    }
  }
  return items;
}
