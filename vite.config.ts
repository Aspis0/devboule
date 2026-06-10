import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { fileURLToPath, URL } from "node:url";

const host = process.env.TAURI_DEV_HOST;

// Extra Rollup entry: the standalone Polis dev harness page. Lets the
// isometric map be built and previewed in a plain browser (no Tauri/login).
//
// SECURITY: the harness page (and the real-repo CityState fixture it bundles)
// must NEVER ship in the production / Tauri bundle. It is only added as a build
// input when POLIS_DEV=1 is set. Default `npm run build` / `tauri build` emit
// the main app only — no polis-dev.html, no repo dump in dist/.
const mainInput = fileURLToPath(new URL("./index.html", import.meta.url));
const polisDevInput = fileURLToPath(new URL("./polis-dev.html", import.meta.url));
const polisDevEnabled = process.env.POLIS_DEV === "1";

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
      input: polisDevEnabled
        ? { main: mainInput, "polis-dev": polisDevInput }
        : { main: mainInput },
      output: {
        manualChunks(id) {
          if (!id.includes("node_modules")) {
            return undefined;
          }
          if (/[\\/]node_modules[\\/](react|react-dom|scheduler)[\\/]/.test(id)) {
            return "react";
          }
          // Keep PixiJS + pixi-viewport out of the generic vendor chunk so the
          // Polis lazy chunk stays isolated and the initial bundle is lean.
          if (/[\\/]node_modules[\\/](pixi\.js|pixi-viewport|pixi-filters|@pixi)[\\/]/.test(id)) {
            return "pixi";
          }
          // Same for the xterm terminal runtime: its own chunk so the in-app
          // agent terminal viewer (lazy) never bloats the initial bundle. This
          // routes the WHOLE @xterm scope (core + addons + CSS) into one chunk;
          // it controls grouping, NOT laziness. Laziness depends on @xterm being
          // reached ONLY via the dynamic import("./createTerminalView"). A future
          // STATIC import of @xterm (or of createTerminalView) anywhere in the
          // eager graph would silently fold this chunk into the initial bundle —
          // this rule would NOT prevent that. Keep the only entry point dynamic.
          if (/[\\/]node_modules[\\/]@xterm[\\/]/.test(id)) {
            return "xterm";
          }
          return "vendor";
        },
      },
    },
  },
});
