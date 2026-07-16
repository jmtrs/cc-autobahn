import { defineConfig } from "vite";

// Tauri expects a fixed dev port and no screen-clearing so Rust logs survive.
export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
});
