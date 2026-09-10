import { resolve, sep } from "path";
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

// ── IPC timing shim ────────────────────────────────────────────────────────
// Every `invoke` in the app — the ~100 modules that import it and the seven
// `@tauri-apps/plugin-*` packages that import it too — resolves to
// `app/tauriCoreProbe.ts` instead of the real module, which re-exports
// everything and times `invoke`. See that file for why the previous approach
// (wrapping `window.__TAURI_INTERNALS__.invoke` at runtime) is impossible:
// Tauri defines that property non-writable AND non-configurable.
//
// Two aliases, not one: the shim itself has to reach the real module, and it
// cannot ask for `@tauri-apps/api/core` (that is what is aliased) nor for
// `@tauri-apps/api/core.js` (the package's `"./*"` export map would turn that
// into `core.js.js`). So it imports the `-real` specifier, resolved here to
// the actual file. Regex-anchored so neither alias catches the other.
const TAURI_CORE_REAL = resolve(__dirname, "node_modules/@tauri-apps/api/core.js");
export const TAURI_CORE_SHIM = resolve(__dirname, "app/tauriCoreProbe.ts");

export const tauriCoreProbe = [
  { find: /^@tauri-apps\/api\/core-real$/, replacement: TAURI_CORE_REAL },
  { find: /^@tauri-apps\/api\/core$/, replacement: TAURI_CORE_SHIM },
];

// The alias above only catches the *bare* specifier. `@tauri-apps/api`'s own
// modules (event, window, path, …) reach `invoke` through a relative
// `./core.js`, which would slip past it — and leave a second, unwrapped copy of
// core in the bundle. Redirect those too, identified by their importer. The
// shim's own `-real` import is exempt because its importer is the shim, not a
// file inside the package.
export function tauriCoreProbePlugin() {
  return {
    name: "fleet-tauri-core-probe",
    enforce: "pre" as const,
    resolveId(source: string, importer: string | undefined) {
      if (!importer || !source.endsWith("./core.js")) return null;
      if (!importer.includes(`${sep}@tauri-apps${sep}api${sep}`)) return null;
      return TAURI_CORE_SHIM;
    },
  };
}

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [tauriCoreProbePlugin(), react()],

  resolve: { alias: tauriCoreProbe },

  build: {
    rollupOptions: {
      input: {
        main: resolve(__dirname, "index.html"),
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
    // `fs.allow` 里的 ".." 是必需的，不是保险起见：本包 import 了仓库根的
    // `shared-ts/`（见 app/components/procCommandLabel.ts），而 vite dev 默认只放行
    // 包目录，于是 `tauri dev` / `pnpm dev` 会对那个文件回 403 Restricted。
    // `vite build` 不看这个设置，所以构建是绿的、只有 dev 挂 —— 别把构建绿当成
    // 这条不需要。
    fs: { allow: [".."] },
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
