import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import path from "node:path";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: { "@": path.resolve(import.meta.dirname, "./src") },
  },
  build: {
    // The Rust service serves this directory.
    outDir: "dist",
    emptyOutDir: true,
  },
  server: {
    port: 5173,
    // Dev proxy: the frontend hot-reloads while the API runs for real, so the
    // whole app is same-origin in development too and cookies behave the same
    // way they will in production.
    proxy: {
      "/api": { target: "http://127.0.0.1:8642", changeOrigin: false },
      "/ws": { target: "ws://127.0.0.1:8642", ws: true },
    },
  },
});
