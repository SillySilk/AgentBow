import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  base: "./",
  // Use our own (empty) PostCSS config so the build never walks up the tree and
  // inherits an ancestor tailwind/postcss config. This app ships plain, local CSS.
  css: { postcss: {} },
  build: { outDir: "dist", emptyOutDir: true },
  server: {
    proxy: {
      "/api": "http://127.0.0.1:9357",
      "/ws": { target: "ws://127.0.0.1:9357", ws: true },
    },
  },
});
