import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";
import pkg from "./package.json" with { type: "json" };

// Dev-time: point the WS/api at a locally running fleet-relay via
// `VITE_RELAY_URL=http://127.0.0.1:18080 pnpm dev`; production builds are
// served by fleet-relay itself, so same-origin needs no config.
export default defineConfig({
  plugins: [react()],
  server: { host: true },
  test: { setupFiles: ["./vitest.setup.ts"] },
  // Surfaced in the 更多 tab's 关于 section.
  define: { __APP_VERSION__: JSON.stringify(pkg.version) },
});
