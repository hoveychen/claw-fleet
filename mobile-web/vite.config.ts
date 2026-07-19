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
export default defineConfig({
  plugins: [react()],
  server: { host: true, headers: securityHeaders() },
  preview: { headers: securityHeaders() },
  test: { setupFiles: ["./vitest.setup.ts"] },
  // Surfaced in the 更多 tab's 关于 section.
  define: {
    __APP_VERSION__: JSON.stringify(pkg.version),
    __APP_COMMIT__: JSON.stringify(buildCommit()),
  },
});

function securityHeaders(): Record<string, string> {
  return {
    "Content-Security-Policy": "default-src 'self'; connect-src 'self' http://localhost:* ws://localhost:* http://127.0.0.1:8098 https://fleet-cloud.muveeai.com; img-src 'self' data: blob:; style-src 'self' 'unsafe-inline'; font-src 'self' data:; frame-src https://fleet-cards.muveeai.com; frame-ancestors https://fleet-pilot.muveeai.com http://localhost:5173; base-uri 'none'; object-src 'none'",
    "Referrer-Policy": "strict-origin",
    "X-Content-Type-Options": "nosniff",
  };
}
