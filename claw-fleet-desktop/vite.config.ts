import { resolve } from "path";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// ── Live-probe proxy (browser harness only) ─────────────────────────────────
// `?mock&live` makes the app's data commands fetch `/__live/*` instead of
// answering from fixtures. This forwards those to a real `fleet serve` and
// adds the bearer token, so the page stays same-origin (the probe emits no
// CORS headers) and never sees the token. Dev-server only — `vite build`
// ignores `server.proxy`, so nothing ships.
// @ts-expect-error process is a nodejs global
const liveProbe = process.env.FLEET_LIVE_PROBE;
// @ts-expect-error process is a nodejs global
const liveToken = process.env.FLEET_LIVE_TOKEN ?? "";
const liveProxy = liveProbe
  ? {
      "/__live": {
        target: liveProbe,
        changeOrigin: true,
        rewrite: (p: string) => p.replace(/^\/__live/, ""),
        headers: { Authorization: `Bearer ${liveToken}` },
      },
    }
  : undefined;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [react()],

  build: {
    rollupOptions: {
      input: {
        main: resolve(__dirname, "index.html"),
        settings: resolve(__dirname, "settings.html"),
        preview: resolve(__dirname, "preview.html"),
        "decision-float": resolve(__dirname, "decision-float.html"),
      },
    },
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    proxy: liveProxy,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching Rust source
      ignored: ["**/src/**", "**/target/**"],
    },
  },
}));
