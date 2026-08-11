/**
 * The single IPC boundary for the frontend.
 *
 * All Rust communication goes through the typed wrappers here — components
 * never call `invoke` directly. The wrapper set grows with each milestone
 * (open_repository, sessions, send_prompt, …) but the boundary stays in one
 * place so the Tauri surface is easy to audit.
 */
import { invoke } from "@tauri-apps/api/core";

export const ipc = {
  /** The desktop app version, proving the Rust ↔ React round trip. */
  getVersion: () => invoke<string>("get_version"),
};
