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
// `--mode webui` 出的是**另一份产物**，不是改默认那份。
//
// 现有的 `dist/` 有三个消费方都按根路径假设：fleet-relay 镜像发到 `/srv/static`、
// 鸿蒙壳同步进 rawfile、Capacitor 打进原生包。给默认构建改 base 会一次性弄坏
// 这三条。所以同源形态另出 `dist-webui/`，挂在 `/m/` 下，三方原样不动。
//
// `VITE_FLEET_HOST` 是让 hostMode.ts 里那几个常量成为编译期常量的东西 ——
// relay 客户端进不进 webui 产物，全靠它能被常量折叠（见 main.tsx 的动态 import）。
export default defineConfig(({ mode }) => {
  const webui = mode === "webui";
  return {
    plugins: [react()],
    server: { host: true },
    test: { setupFiles: ["./vitest.setup.ts"] },
    base: webui ? "/m/" : "/",
    build: webui ? { outDir: "dist-webui" } : {},
    // Surfaced in the 更多 tab's 关于 section.
    define: {
      __APP_VERSION__: JSON.stringify(pkg.version),
      __APP_COMMIT__: JSON.stringify(buildCommit()),
      "import.meta.env.VITE_FLEET_HOST": JSON.stringify(webui ? "webui" : "relay"),
    },
  };
});
