import { defineConfig } from "vite";

// Tauri expects a fixed dev-server port and a build output the bundler can find.
// See: https://v2.tauri.app/start/frontend/vite/
export default defineConfig({
  clearScreen: false,
  optimizeDeps: {
    exclude: ["occt-wasm"],
  },
  worker: {
    format: "es",
  },
  server: {
    port: 1420,
    strictPort: true,
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: "esnext",
    outDir: "dist",
  },
});
