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

**Milestone 2 of 14 — workspace skeleton, core types, and DeepSeek request/response types — complete.**

Implemented:

- Cargo workspace with four crates (`vava-core`, `vava-deepseek`,
  `vava-coding`, `vava-cli`) mirroring the four conceptual layers
- Provider-independent conversation types: `Message`, `UserMessage`,
  `AssistantMessage`, `ToolCall`, `ToolResultMessage`, `ToolDefinition`
- `reasoning_content` treated as a first-class part of assistant messages
  (it survives serialization by design)
- Typed events: `ModelEvent` (streaming model output) and `AgentEvent`
  (the contract between the agent and any frontend)
- Core error types: `ToolError`, `Cancelled`
- DeepSeek request serialization (`DeepSeekRequest`, explicit wire types,
  `thinking` parameter, tool-call arguments as JSON strings)
- DeepSeek response types: `ChatResponse`, `StreamChunk`, `Delta`,
  `DeepSeekUsage`, `ApiErrorBody`
- `ModelConfig` (model, thinking mode, base URL) with official-API defaults
- Unit tests for serialization shapes, round-trips, response parsing, and
  fragmented tool-call argument accumulation

Not yet implemented: the DeepSeek HTTP client, the SSE parser, tools, the
agent loop, the CLI, session persistence. See [Roadmap](#roadmap).

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

Planned, in order:

- [ ] **M3** SSE streaming parser (fixture-based tests)
- [ ] **M4** `Tool` trait and `ToolRegistry` (fake tools first)
- [ ] **M5** Agent/tool-call loop against a fake model
- [ ] **M6** Real DeepSeek client — `vava -p "say hello"` works
- [ ] **M7** `read` tool
- [ ] **M8** `write`, `edit`, `bash` tools; a real coding task works
- [ ] **M9** `AgentHarness` with cancellation
- [ ] **M10** `CodingSession`: repository root, `AGENTS.md`, system prompt
- [ ] **M11** JSONL session persistence (`~/.local/share/vava/sessions/`)
- [ ] **M12** Improved CLI rendering (debug/verbose reasoning output)
- [ ] **M13** Interactive REPL (`vava` with no prompt)
- [ ] **M14** Ratatui TUI consuming the same `AgentEvent` stream

Deliberately out of scope for v1: OpenAI/Anthropic/OpenRouter support, OAuth,
databases, web servers, multi-provider abstractions, context compaction,
session resume/branching, image input, permission prompts, LSP/tree-sitter
aware tools. The architecture leaves room for these, but they are not built
speculatively.
