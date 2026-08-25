/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** 谁托管这份 bundle:`"relay"`(缺省)或 `"webui"`。见 hostMode.ts —— 它
   *  必须是编译期常量,否则 relay 客户端会被打进同源构建里。 */
  readonly VITE_FLEET_HOST?: string;
  /** 开发期指向本地 relay 的覆盖值(relay 形态用)。 */
  readonly VITE_RELAY_URL?: string;
}

/** App version, injected by vite `define` from package.json (see vite.config.ts). */
declare const __APP_VERSION__: string;

/** Short git commit this bundle was built from, injected by vite `define` (see
 *  vite.config.ts `buildCommit`). Reported to the desktop in `client_hello` so
 *  it can flag a phone running a stale deploy. `"unknown"` when neither
 *  `VITE_APP_COMMIT` nor a git checkout was available at build time. */
declare const __APP_COMMIT__: string;
