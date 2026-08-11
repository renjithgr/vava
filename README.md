# vava

A minimal, understandable coding agent written in Rust, powered by the
DeepSeek API.

vava is designed so that the *entire architecture* can be understood by
reading the source code: from `main()` through `CodingSession`,
`AgentHarness`, `DeepSeekClient`, and `ToolRegistry`, without a large
framework in the way.

## What vava is

vava is a command-line agent that works inside a software repository. You ask
it to do something — explain code, run tests, fix a failure — and it does so
by calling a small set of tools (`read`, `write`, `edit`, `bash`) in a loop
until it has an answer. Model output streams to your terminal as it arrives.

## Why it exists

Large agent frameworks solve many problems at once: multi-provider support,
context management, RAG, permission systems, plugins, memory, and more. That
power comes with complexity. vava is the opposite experiment: keep the
feature set deliberately small so the control flow — model → tool → result →
model → final answer — is visible in one reading of the code.

Architectural inspiration comes from Hugging Face's `tau`, but vava is not a
port and supports a much smaller feature set.

## Current status

**Milestone 11 of 14 — JSONL session persistence — complete.**

Implemented:

- `SessionStore` / `SessionLog`: append-only JSONL logs under the platform
  data dir (`~/.local/share/vava/sessions/` on Linux, `~/Library/Application
  Support/vava/sessions/` on macOS)
- Header record `{"type":"session","id":...,"cwd":...}`; message records
  reuse the core serde representation, so `reasoning_content` survives by
  construction
- `CodingSession` wires a message sink into the harness: every completed
  message is appended synchronously as it happens (tiny write + flush)
- Replay reconstructs the transcript, tolerates a truncated final line
  (crash mid-write), and reports corruption of middle records; `list`
  enumerates sessions for the future `--resume`
- 9 new tests: header shape, append/replay round trips, reasoning
  survival, truncated-final-line tolerance, corruption detection, missing
  sessions, listing, exact JSONL format, and a live session replay that
  matches the transcript

Not yet implemented: improved rendering, the REPL. See [Roadmap](#roadmap).

## Installation

Requires Rust 1.85+ (edition 2024).

```bash
git clone <this-repo> && cd vava
cargo build --release
# binary at target/release/vava
```

## Configuration

`vava` authenticates with a DeepSeek API key:

```bash
export DEEPSEEK_API_KEY="sk-..."
```

Additional configuration (model, thinking mode, base URL) will be exposed as
command-line flags as those features land. The base URL defaults to the
official DeepSeek API.

## Basic examples

```bash
# One-shot prompt
vava -p "Explain this repository"

# Prompt in a specific directory
vava --cwd /path/to/repo -p "Run the tests and fix them"
```

An interactive REPL (`vava` with no prompt) is planned for a later milestone.

## Architecture

Four layers, each with a single responsibility:

```text
CLI / TUI
    │
    ▼
CodingSession
    │
    ▼
AgentHarness
    │
    ▼
DeepSeekClient
```

| layer          | crate           | knows about                                        | never does                     |
|----------------|-----------------|----------------------------------------------------|--------------------------------|
| CLI / TUI      | `vava-cli`      | rendering, input, agent events                     | talk to DeepSeek, touch files  |
| CodingSession  | `vava-coding`   | repository root, `AGENTS.md`, system prompt, tools, sessions | render output          |
| AgentHarness   | `vava-core`     | conversation transcript, tool loop, cancellation   | print, know the repository     |
| DeepSeekClient | `vava-deepseek` | the DeepSeek wire protocol, SSE, token usage       | execute tools, touch files     |

The mental model:

```text
                    repository-specific
                           │
                           ▼
                     CodingSession
                           │
                           ▼
CLI / TUI ──────── AgentHarness
    ▲                    │
    │                    ▼
    └──── AgentEvent ─ Agent Loop
                           │
                  ┌────────┴────────┐
                  ▼                 ▼
            DeepSeekClient      ToolRegistry
                                      │
                             ┌────────┼────────┐
                             ▼        ▼        ▼
                           read      edit     bash
```

Key rules:

- `vava-core` message types are provider-independent. DeepSeek-specific
  serialization lives entirely in `vava-deepseek`; nothing in `vava-core`
  carries `#[serde]` annotations tuned to the DeepSeek wire format. The
  core's serde shape is the *persistence* format (JSONL session logs).
- `AgentEvent` is the only channel between the harness and any frontend.
  The print CLI, the REPL, and the future Ratatui TUI all consume the same
  events. The harness never prints.
- `reasoning_content` is a first-class part of assistant messages: it
  survives streaming, tool-call loops, persistence, and restoration, because
  DeepSeek may need it on subsequent tool turns.

## Security model

Two distinct trust domains:

**Filesystem tools (`read`, `write`, `edit`).** These are deliberately
sandboxed to the repository root. Every path is resolved against the root,
canonicalized where possible, and verified to remain under the root before
any operation. `../`, absolute paths, and symlinks are handled explicitly.
The goal is that a confused model cannot accidentally read or modify files
outside the repository.

**The `bash` tool.** This is intentionally *not* sandboxed. It runs
arbitrary commands with your user's permissions via your shell, from the
repository root. That is the point: vava cannot run your test suite, install
dependencies, or use git otherwise. **vava is not a security sandbox.** The
model is given the power to do anything you could do in a terminal, and you
should treat it accordingly. Do not run vava on a repository you would not
trust with a human who has your shell access.

## Development

```bash
cargo fmt
cargo clippy --workspace --all-targets
cargo test --workspace
```

Logging uses `tracing`; enable debug output with:

```bash
RUST_LOG=vava=debug vava -p "…"
```

API keys and authorization headers are never logged.

## Roadmap

Implemented:

- [x] **M1** Workspace skeleton; `Message`, `ToolCall`, `ToolResult`,
      `ModelEvent`, `AgentEvent`, error types
- [x] **M2** DeepSeek request serialization and response types
- [x] **M3** SSE streaming parser (fixture-based tests)
- [x] **M4** `Tool` trait and `ToolRegistry` (fake tools first)
- [x] **M5** Agent/tool-call loop against a fake model
- [x] **M6** Real DeepSeek client — `vava -p "say hello"` works
- [x] **M7** `read` tool
- [x] **M8** `write`, `edit`, `bash` tools; a real coding task works
- [x] **M9** `AgentHarness` with cancellation
- [x] **M10** `CodingSession`: repository root, `AGENTS.md`, system prompt
- [x] **M11** JSONL session persistence (`~/.local/share/vava/sessions/`)

Planned, in order:

- [ ] **M12** Improved CLI rendering (debug/verbose reasoning output)
- [ ] **M13** Interactive REPL (`vava` with no prompt)
- [ ] **M14** Ratatui TUI consuming the same `AgentEvent` stream

Deliberately out of scope for v1: OpenAI/Anthropic/OpenRouter support, OAuth,
databases, web servers, multi-provider abstractions, context compaction,
session resume/branching, image input, permission prompts, LSP/tree-sitter
aware tools. The architecture leaves room for these, but they are not built
speculatively.
