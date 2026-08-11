# vava desktop — implementation plan

Working plan for the vava desktop app (`apps/vava-desktop`): a native Tauri 2
frontend over the existing Rust agent, sharing the exact same agent behavior
as the CLI. This document records decisions that are already baked in, the
current status, and the concrete roadmap so the work can be picked up later
without re-deriving the architecture.

## Status

| Milestone | State | Notes |
|-----------|-------|-------|
| D1 skeleton | ✅ done | Tauri 2 + React + TS + Vite, `get_version` IPC |
| D2 open repository | ✅ done | folder picker, root resolution, CodingSession, recents |
| D3 sessions | ✅ done | sidebar, list/select/new/resume, transcript render |
| D4 prompt streaming | ✅ done | send_prompt, Tauri Channel, live text, turn lifecycle |
| D5 tool cards | ✅ done | read/write/edit/bash cards, transcript normalization |
| D6 cancellation polish | ⬜ next | Stop exists; add Esc, visible "(cancelled)" state |
| D7 diffs + Changes panel | ⬜ | needs a diff source in Rust first (see below) |
| D8 @file completion | ⬜ | needs a file index in Rust first (see below) |
| D9 settings + keychain | ⬜ | env-key only today |
| D10 UX polish | ⬜ | shortcuts, markdown, window state |

## How to run and test

```bash
# app (dev mode with the DeepSeek key)
cd apps/vava-desktop
DEEPSEEK_API_KEY="sk-..." npm run tauri dev

# checks
cargo fmt --check && cargo clippy --workspace --all-targets --all-features && cargo test --workspace
npm run typecheck && npm run lint && npm run build && npm test
```

The API key is read from `DEEPSEEK_API_KEY` once at startup
(`src-tauri/src/state.rs`, `model_client_from_env`). Without it, repos and
sessions are still browsable; prompts fail with "DeepSeek API key is not
configured." D9 replaces the env source with keychain-backed settings.

## Architecture map (do not break these boundaries)

```
React (UI only)
  └─ lib/ipc.ts            single typed invoke boundary; components never call invoke
      └─ Tauri commands    thin adapters only (src-tauri/src/commands.rs)
          └─ DesktopState  (src-tauri/src/state.rs) — repo, session, running turn
              ├─ events.rs   AgentEvent → DesktopAgentEvent (Rust → React stream)
              ├─ model.rs    IPC DTOs (RepositoryInfo, SessionView, DesktopMessage)
              ├─ errors.rs   DesktopError → { kind, message }
              └─ vava-coding / vava-core / vava-deepseek   (untouched by Tauri)
```

- **Wire conventions** (tested, keep them): DTOs are camelCase on the wire;
  transcript messages mirror vava's JSONL persistence shape (snake_case);
  event payloads are camelCase (`callId`, `isError`).
- **Session store location**: `SessionStore::open_at` for tests, platform dir
  (`~/Library/Application Support/vava/sessions`) in production; the desktop
  holds one `SessionStore` on `DesktopState`.
- **Turn lifecycle** (`state.rs`): only one turn at a time. `send_prompt`
  takes the `CodingSession` out of state, spawns `run_turn` (owns the
  session, writes it back, signals a `watch`), and `stop_running_turn`
  cancels + awaits the write-back before any switch/new turn. Tests in
  `state.rs` cover streaming, cancellation, and stale-session rejection.
- **Model client**: built once at startup from the env (`client` field on
  `DesktopState`). D9 needs a rebuild path when settings change.
- **Tool result formats** (parsed by the frontend, `lib/tools.ts`):
  - read: `{ path, offset?, limit? }` → numbered file content
  - write: `{ path, content }` → "wrote N bytes to `path`"
  - edit: `{ path, old_text, new_text }` → "edited `path`"
  - bash: `{ command }` → `[stdout]… [stderr]… exit code: N duration: X.XXs`

## Frontend state (stores)

- `stores/repository.ts` — active repo, recents, open/remove actions.
- `stores/session.ts` — sessions list, active transcript, and live streaming
  (`running`, `streaming`, `toolResults`, `epoch` guard for stale turns).
  Narrow zustand selectors keep text deltas from re-rendering the whole
  conversation. Tested by Vitest (`npm test`).

## Remaining milestones

### D6 — cancellation polish
Goal: Stop is already wired (`cancel_turn` → harness streams
`error: "operation cancelled"`; the store ends the turn without a banner).
Finish the UX:
- Esc (and Cmd/Ctrl+.) to stop, per Phase 12; document it.
- Show a visible "(cancelled)" status on the last assistant bubble instead
  of silently clearing it.
- Tests: cancellation state already covered in `stores/session.test.ts`
  ("cancellation does not surface as an error banner", "stop cancels…").

### D7 — diffs + Changes panel
Important: vava has **no diff computation anywhere today**; the edit card
uses a temporary frontend `simpleDiff` preview (`lib/tools.ts`). Per the
spec's boundary rule, diff computation belongs in the Rust reusable layer.
- Add a `vava-coding` git/diff module (e.g. `crates/vava-coding/src/diff.rs`)
  that computes `FileDiff { path, added, removed, patch }`. Decision needed:
  shell out to the `git` CLI (needs care: it is the repo's own git, not a
  vava child process) or add a pure-Rust diff crate (e.g. `similar`).
- Expose via a thin Tauri command (e.g. `changed_files` / `file_diff`);
  the frontend never shells out.
- Replace the `simpleDiff` preview with the Rust `patch` render.
- Changes panel: collapsible right-side panel listing changed files
  (M/A status, +N/−M), clicking opens the diff. Falls back gracefully
  outside Git. No commit/reset (out of scope).

### D8 — @file completion
Important: vava has **no file index anywhere today** (the system prompt
deliberately injects no tree). The desktop must not crawl the repo in
React.
- Build the index in `vava-coding` (walk the workspace root, respect
  `.gitignore`/common ignores, keep it cheap; no second crawler).
- Fuzzy search + ranking in Rust (single place, usable by CLI later).
- Tauri command: `search_repository_files(query, limit)` → `Vec<FileMatch>`.
- Frontend: `@` trigger in `PromptEditor`, dropdown with fuzzy results,
  insert `@path` into the prompt (the model receives it as text; no special
  protocol).

### D9 — settings + keychain
- Settings screen: DeepSeek API key (masked, never sent back to React —
  only "configured: yes/no"), model, thinking toggle, Test Connection, Save.
- Store the key via the OS keychain (macOS Keychain / Windows Credential
  Manager / Linux Secret Service). Decision needed: `keyring` crate vs a
  Tauri plugin. macOS: expect a keychain access prompt; dev builds without
  signing may need `codesign` or a self-signed identity.
- Add a rebuild path for the model client on `DesktopState` (currently
  built once at startup), and re-open/re-point the active CodingSession.
- No provider dropdown; DeepSeek only (Phase 20).

### D10 — UX polish
- Keyboard shortcuts (Phase 28): Cmd/Ctrl+O open repo, Cmd/Ctrl+N new
  session, Cmd/Ctrl+K focus prompt, Esc close/cancel. Small helper module;
  no shortcut customization system.
- Markdown rendering for assistant text (code blocks, inline code, lists,
  headings, links) + syntax highlighting for fenced blocks; render-only —
  the textual transcript stays intact (Phase 9).
- Loading/empty states; window state persistence (size/position, sidebar
  width, last opened repo) via the same lightweight JSON-file approach as
  recents or `tauri-plugin-window-state` (Phase 29).
- Structure conversation components so virtualization can be added later if
  needed (Phase 25) — don't add it preemptively.
- Context footer (`deepseek-chat · context/tokens`) only if/when the Rust
  layer exposes usage (Phase 21) — today `Usage` exists in events but is
  not surfaced; no separate TS token accounting.

## Explicitly out of scope (do not add)

Multiple providers, MCP, extensions/plugins, LSP, subagents, cloud sync,
local HTTP server, Electron, commits/reset UI, GitHub integration, terminal
emulator, context compaction (not in core vava), steering (not in core
vava), LLM-generated session titles.

## Definition of first usable release (checklist)

- [ ] Open repo from recents or folder picker
- [ ] New session / resume previous / continue after relaunch
- [ ] Streamed prompt with visible tool cards and Stop/Esc cancellation
- [ ] Changed files visible with real diffs
- [ ] @file references in the prompt
- [ ] API key configured through the UI (keychain), not just env
- [ ] All checks green: 217+ Rust tests, 26+ frontend tests, fmt/clippy,
      typecheck/lint/build
