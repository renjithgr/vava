import { describe, expect, it } from "vitest";
import {
  countLines,
  isRecord,
  parseBashOutput,
  simpleDiff,
  stringArg,
} from "./tools";

describe("parseBashOutput", () => {
  it("extracts stdout, stderr, exit code, and duration", () => {
    const parsed = parseBashOutput(
      "[stdout]\nline one\nline two\n[stderr]\nerror text\nexit code: 0\nduration: 2.40s\n",
    );
    expect(parsed.stdout).toBe("line one\nline two");
    expect(parsed.stderr).toBe("error text");
    expect(parsed.exitCode).toBe(0);
    expect(parsed.duration).toBe("2.40s");
    expect(parsed.timedOut).toBe(false);
    expect(parsed.truncated).toBe(false);
    expect(parsed.matches).toBe(true);
  });

  it("handles a failing command and timeout markers", () => {
    const parsed = parseBashOutput(
      "[stderr]\nboom\nexit code: 101\nduration: 0.10s\ntimeout: true\n(output truncated)\n",
    );
    expect(parsed.stderr).toBe("boom");
    expect(parsed.stdout).toBe("");
    expect(parsed.exitCode).toBe(101);
    expect(parsed.timedOut).toBe(true);
    expect(parsed.truncated).toBe(true);
  });

  it("reports killed processes with a null exit code", () => {
    const parsed = parseBashOutput("exit code: killed\nduration: 3.00s\n");
    expect(parsed.exitCode).toBeNull();
    expect(parsed.matches).toBe(true);
  });

  it("flags unrecognized output so the card can fall back", () => {
    const parsed = parseBashOutput("garbage output");
    expect(parsed.matches).toBe(false);
    expect(parsed.exitCode).toBeNull();
  });
});

describe("tool argument helpers", () => {
  it("reads string arguments defensively", () => {
    expect(stringArg({ path: "src/main.rs" }, "path")).toBe("src/main.rs");
    expect(stringArg({ path: 42 }, "path")).toBeNull();
    expect(stringArg(null, "path")).toBeNull();
    expect(stringArg(undefined, "path")).toBeNull();
  });

  it("recognizes plain objects only", () => {
    expect(isRecord({})).toBe(true);
    expect(isRecord(null)).toBe(false);
    expect(isRecord([1])).toBe(false);
    expect(isRecord("x")).toBe(false);
  });

  it("counts lines without counting a trailing newline", () => {
    expect(countLines("")).toBe(0);
    expect(countLines("a\nb")).toBe(2);
    expect(countLines("a\nb\n")).toBe(2);
  });
});

describe("simpleDiff", () => {
  it("splits removed and added lines", () => {
    const { removed, added } = simpleDiff(
      "let a = 1;\nlet b = 2;\n",
      "let a = 1;\nlet b = 3;\n",
    );
    expect(removed).toEqual(["let b = 2;"]);
    expect(added).toEqual(["let b = 3;"]);
  });

  it("handles empty sides", () => {
    expect(simpleDiff("", "new line\n")).toEqual({
      removed: [],
      added: ["new line"],
    });
    expect(simpleDiff("old line\n", "")).toEqual({
      removed: ["old line"],
      added: [],
    });
  });
});
