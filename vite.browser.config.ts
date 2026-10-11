import { defineConfig } from "vite";

export default defineConfig({
  plugins: [
    {
      name: "browser-entry-name",
      enforce: "post",
      generateBundle(_options, bundle) {
        const entry = bundle["browser/index.html"];
        if (!entry) return;
        delete bundle["browser/index.html"];
        entry.fileName = "index.html";
        bundle["index.html"] = entry;
      },
    },
  ],
  clearScreen: false,
  optimizeDeps: { exclude: ["occt-wasm"] },
  worker: { format: "es" },
  build: {
    target: "esnext",
    outDir: "dist-browser",
    rollupOptions: { input: "browser/index.html" },
  },
});
