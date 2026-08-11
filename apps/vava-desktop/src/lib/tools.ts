/**
 * Presentation helpers for tool cards (D5).
 *
 * These parse the *display* of tool arguments and results; they are purely
 * presentation. Computing diffs is agent behavior and belongs in the Rust
 * layer (D7) — the `simpleDiff` here is a temporary preview for the edit
 * card until the real unified diff lands.
 */

export interface ToolResultInfo {
  content: string;
  isError: boolean;
}

/** A parsed view of a bash tool result (the CLI's `[stdout]/[stderr]` format). */
export interface ParsedBashOutput {
  stdout: string;
  stderr: string;
  /** The process exit code; null when the process was killed. */
  exitCode: number | null;
  /** The reported duration, e.g. `1.23s`, when present. */
  duration: string | null;
  timedOut: boolean;
  truncated: boolean;
  /** Whether the output matched the expected format at all. */
  matches: boolean;
}

/** Whether a value is a plain object usable as tool arguments. */
export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Read a string field from tool arguments, if present. */
export function stringArg(args: unknown, key: string): string | null {
  if (!isRecord(args)) return null;
  const value = args[key];
  return typeof value === "string" ? value : null;
}

/** Split content into lines, counting a trailing newline as no extra line. */
export function countLines(content: string): number {
  if (content === "") return 0;
  return content.split("\n").length - (content.endsWith("\n") ? 1 : 0);
}

/** Extract the text between `start` and `end` markers (end exclusive). */
function section(content: string, start: string, end: string): string | null {
  const from = content.indexOf(start);
  if (from < 0) return null;
  const body = content.slice(from + start.length).replace(/^\n/, "");
  const to = end ? body.indexOf(end) : -1;
  const section = to >= 0 ? body.slice(0, to) : body;
  return section.replace(/\n$/, "");
}

/**
 * Parse a bash tool result into stdout/stderr/exit/duration.
 * Returns `matches: false` when the format is unrecognized so the card can
 * fall back to showing the raw content.
 */
export function parseBashOutput(content: string): ParsedBashOutput {
  const stdout = section(content, "[stdout]", "[stderr]");
  const stderr = section(content, "[stderr]", "exit code:");
  const exitMatch = content.match(/exit code: (killed|\d+)/);
  const durationMatch = content.match(/duration: ([0-9.]+s)/);
  const matches = stdout !== null || stderr !== null || exitMatch !== null;

  return {
    stdout: stdout ?? "",
    stderr: stderr ?? "",
    exitCode: exitMatch ? (exitMatch[1] === "killed" ? null : Number(exitMatch[1])) : null,
    duration: durationMatch?.[1] ?? null,
    timedOut: content.includes("timeout: true"),
    truncated: content.includes("(output truncated)"),
    matches,
  };
}

/** Split into lines, treating a single trailing newline as a terminator. */
function splitLines(text: string): string[] {
  if (text === "") return [];
  const trimmed = text.endsWith("\n") ? text.slice(0, -1) : text;
  return trimmed.split("\n");
}

/**
 * A minimal before/after diff for the edit card, computed from the tool's
 * `old_text`/`new_text` arguments. Purely a preview: D7 replaces this with
 * the unified diff computed in the Rust layer.
 */
export function simpleDiff(
  oldText: string,
  newText: string,
): { removed: string[]; added: string[] } {
  const oldLines = splitLines(oldText);
  const newLines = splitLines(newText);
  const removed = oldLines.filter((line) => !newLines.includes(line));
  const added = newLines.filter((line) => !oldLines.includes(line));
  return { removed, added };
}
