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

**Milestone 15 of 15 — session resume & session management — complete.**

Sessions are now actually usable day to day:

- `vava -c` continues the most recent session of the current repository
- `vava -r` lists the repository's sessions and resumes the one you pick
- `vava --resume <id>` resumes a specific session (full id or prefix)
- inside the REPL and TUI: `/new`, `/resume`, and `/session`
- sessions are scoped to a stable hash of the canonicalized repository
  root; sessions written before scoping are still found
- resuming restores the complete transcript, reasoning content and tool
  calls included, so DeepSeek can pick up where you left off

Workflows:

```bash
cd some-rust-project
export DEEPSEEK_API_KEY=...
vava -p "Run the tests, identify the failure, fix it, and run the tests again."
```

with progress streaming to the terminal through `bash`, `read`, `edit`,
`bash`, then a final response — in print, REPL, or TUI mode. Later, from
the same repository, `vava -c` restores the previous conversation.

## Sessions

vava persists every conversation as an append-only JSONL log. Sessions are
scoped to the repository they belong to, so leaving a repository and coming
back later lets you continue right where you stopped.

```bash
# Start (or continue) an interactive session
vava

# Continue the most recent session of this repository
vava -c
vava --continue

# List this repository's sessions and pick one
vava -r
vava --resume

# Resume a specific session (full id or a unique prefix)
vava --resume 01KABC
```

Resume modes are mutually exclusive and cannot be combined with a one-shot
prompt (`vava -c --resume …` and `vava -p "…" --resume` are rejected).
`vava -c -p "…"` is allowed: continue the latest session and run that one
prompt.

Inside the interactive REPL and the TUI:

```text
/new        start a fresh session for the same repository
/resume     list this repository's sessions and switch to one
/session    show the current session's metadata
```

When vava resumes a session it restores the complete transcript — user
messages, assistant messages, reasoning content, tool calls, and tool
results — so the model can continue the task with full context. Sessions
are stored under the platform data directory:

```text
~/.local/share/vava/sessions/<repo-hash>/<session-id>.jsonl
```

where `<repo-hash>` is a stable hash of the canonicalized repository root.
Sessions written before repository scoping (flat files with a `cwd`
header) are still found and listed for their original repository.

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

# Continue the previous conversation
vava -c
```

With no prompt, `vava` opens an interactive REPL; `vava --tui` opens the
full-screen Ratatui frontend.

## Desktop app

In addition to the terminal frontend, vava ships a native desktop app
(`apps/vava-desktop`) built with Tauri 2, React, and TypeScript. It is the
same coding agent with a different user interface: React renders, the Tauri
commands are a thin adapter, and all agent behavior (sessions, tools,
DeepSeek calls, cancellation) comes from the same `vava-coding` /
`vava-core` / `vava-deepseek` crates the CLI uses.

```text
                    vava-core
                        │
                   vava-coding
                   /          \
                  /            \
          vava-cli          vava-desktop
             │                   │
             ▼                   ▼
          Terminal          Tauri + React
```

The desktop app does not wrap the CLI executable and does not run an
internal HTTP server; React and Rust communicate over Tauri IPC only.

```bash
cd apps/vava-desktop
npm install
npm run tauri dev
```

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
  The print CLI, the REPL, and the Ratatui TUI all consume the same
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
- [x] **M12** Improved CLI rendering (debug/verbose reasoning output)
- [x] **M13** Interactive REPL (`vava` with no prompt)
- [x] **M14** Ratatui TUI consuming the same `AgentEvent` stream
- [x] **M15** Session resume & session management (`-c`, `-r`, `--resume`,
      `/new`, `/resume`, `/session`, repository-scoped sessions)
- [x] **D1** Desktop skeleton: Tauri 2 + React + TypeScript + Vite,
      workspace integration, `get_version` IPC round trip
- [x] **D2** Repository opening, recent repositories
- [x] **D3** Sessions: list, select, new, resume
- [x] **D4** Prompt execution with live streaming
- [ ] **D5** Tool cards (read/write/edit/bash)
- [ ] **D6** Cancellation
- [ ] **D7** Diffs and the Changes panel
- [ ] **D8** `@file` completion
- [ ] **D9** DeepSeek settings with secure keychain storage
- [ ] **D10** UX polish: shortcuts, markdown, window state

Deliberately out of scope for v1: OpenAI/Anthropic/OpenRouter support, OAuth,
databases, web servers, multi-provider abstractions, context compaction,
session branching, session deletion UI, image input, permission prompts,
LSP/tree-sitter aware tools. The architecture leaves room for these, but
they are not built speculatively.
