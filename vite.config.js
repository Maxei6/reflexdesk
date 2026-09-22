import { defineConfig } from "vite";
import { resolve } from "node:path";

const isolationHeaders = {
  "Cross-Origin-Opener-Policy": "same-origin",
  "Cross-Origin-Embedder-Policy": "require-corp",
};

export default defineConfig({
  server: {
    host: "127.0.0.1",
    port: 1420,
    strictPort: true,
    headers: isolationHeaders,
  },
  worker: {
    format: "es",
  },
  build: {
    target: "esnext",
    rollupOptions: {
      input: {
        main: resolve(import.meta.dirname, "index.html"),
        overlay: resolve(import.meta.dirname, "overlay.html"),
      },
    },
  },
});
