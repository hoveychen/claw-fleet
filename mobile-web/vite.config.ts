import { execSync } from "node:child_process";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";
import pkg from "./package.json" with { type: "json" };

// The git commit this bundle was built from — reported to the desktop in
// `client_hello` so it can flag a phone running a stale deploy (see relay.ts
// `DeviceInfo.appCommit`). The relay image builds `dist` *inside* Docker where
// `.git` is absent (see fleet-relay/Dockerfile), so CI passes the commit via
// `VITE_APP_COMMIT`; a local `pnpm build` falls back to a live `git` read, and
// a tarball build with neither degrades to "unknown".
function buildCommit(): string {
  const injected = process.env.VITE_APP_COMMIT?.trim();
  if (injected) return injected.slice(0, 7);
  try {
    return execSync("git rev-parse --short=7 HEAD", { stdio: ["ignore", "pipe", "ignore"] })
      .toString()
      .trim();
  } catch {
    return "unknown";
  }
}

// Dev-time: point the WS/api at a locally running fleet-relay via
// `VITE_RELAY_URL=http://127.0.0.1:18080 pnpm dev`; production builds are
// served by fleet-relay itself, so same-origin needs no config.
// `--mode webui` produces a **separate artifact**, not a variant of the default.
//
// Current `dist/` has three consumers all assuming a root-path deploy: fleet-relay image to `/srv/static`,
// HarmonyOS shell synced into rawfile, and Capacitor bundled into the native app. Changing the base for
// the default build breaks all three at once. So the same-origin variant outputs separately to `dist-webui/`,
// mounted at `/m/`, leaving the three consumers unchanged.
//
// `VITE_FLEET_HOST` lets the constants in hostMode.ts become compile-time constants—
// whether relay clients see the webui artifact at all depends on it being constant-folded (see main.tsx's dynamic import).
export default defineConfig(({ mode }) => {
  const webui = mode === "webui";
  return {
    plugins: [react()],
    // `fs.allow` ".." is required, not optional: this package imports `shared-ts/` from the repo root
    // (see src/views/TerminalView.tsx), but vite dev only allows the package dir by default,
    // so `pnpm dev` returns 403 Restricted for those files. `vite build` ignores this setting,
    // so build is green while dev breaks—don't mistake a green build for not needing this.
    server: { host: true, fs: { allow: [".."] } },
    test: { setupFiles: ["./vitest.setup.ts"] },
    base: webui ? "/m/" : "/",
    build: webui ? { outDir: "dist-webui" } : {},
    // Surfaced in the "更多" (More) tab's "关于" (About) section.
    define: {
      __APP_VERSION__: JSON.stringify(pkg.version),
      __APP_COMMIT__: JSON.stringify(buildCommit()),
      "import.meta.env.VITE_FLEET_HOST": JSON.stringify(webui ? "webui" : "relay"),
    },
  };
});
