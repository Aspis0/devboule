import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { fileURLToPath, URL } from "node:url";

const host = process.env.TAURI_DEV_HOST;

// Main app entry only. (Standalone polis-dev harness was removed for public tree;
// map still runs in-app via Tauri.)
const mainInput = fileURLToPath(new URL("./index.html", import.meta.url));

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: "esnext",
    minify: !process.env.TAURI_DEBUG ? "esbuild" : false,
    sourcemap: !!process.env.TAURI_DEBUG,
    rollupOptions: {
      input: { main: mainInput },
      output: {
        manualChunks(id) {
          if (!id.includes("node_modules")) {
            return undefined;
          }
          if (/[\\/]node_modules[\\/](react|react-dom|scheduler)[\\/]/.test(id)) {
            return "react";
          }
          if (/[\\/]node_modules[\\/](pixi\.js|pixi-viewport|pixi-filters|@pixi)[\\/]/.test(id)) {
            return "pixi";
          }
          if (/[\\/]node_modules[\\/]@xterm[\\/]/.test(id)) {
            return "xterm";
          }
          return "vendor";
        },
      },
    },
  },
});
