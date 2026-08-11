import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri expects a fixed dev server port, and `clearScreen: false` keeps
// Rust compiler output visible while the dev server runs.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      // Avoid the dev server reloading when Rust code changes.
      ignored: ["**/src-tauri/**"],
    },
  },
});
