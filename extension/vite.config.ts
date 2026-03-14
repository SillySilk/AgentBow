import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { resolve } from "path";
import { copyFileSync, mkdirSync, readdirSync } from "fs";

// Plugin to copy manifest.json and icons into dist after build
function copyExtensionAssets() {
  return {
    name: "copy-extension-assets",
    closeBundle() {
      // Copy manifest.json
      copyFileSync(
        resolve(__dirname, "manifest.json"),
        resolve(__dirname, "dist/manifest.json")
      );
      // Copy icons
      const iconsDir = resolve(__dirname, "icons");
      const distIcons = resolve(__dirname, "dist/icons");
      try {
        mkdirSync(distIcons, { recursive: true });
        for (const file of readdirSync(iconsDir)) {
          copyFileSync(resolve(iconsDir, file), resolve(distIcons, file));
        }
      } catch {}
    },
  };
}

export default defineConfig({
  plugins: [react(), copyExtensionAssets()],
  build: {
    outDir: "dist",
    emptyOutDir: true,
    rollupOptions: {
      input: {
        sidepanel: resolve(__dirname, "src/sidepanel/index.html"),
        "service-worker": resolve(__dirname, "src/background/service-worker.ts"),
        "page-extractor": resolve(__dirname, "src/content/page-extractor.ts"),
      },
      output: {
        entryFileNames: (chunk) => {
          if (chunk.name === "service-worker") return "background/[name].js";
          if (chunk.name === "page-extractor") return "content/[name].js";
          return "[name].js";
        },
        chunkFileNames: "chunks/[name]-[hash].js",
        assetFileNames: (asset) => {
          if (asset.name?.endsWith(".css")) return "styles/[name][extname]";
          return "assets/[name][extname]";
        },
      },
    },
  },
  resolve: {
    alias: {
      "@": resolve(__dirname, "src"),
    },
  },
});
