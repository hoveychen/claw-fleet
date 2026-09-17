/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** Who hosts this bundle: `"relay"` (default) or `"webui"`. See hostMode.ts — it
   *  must be a compile-time constant, otherwise the relay client gets bundled into same-origin builds. */
  readonly VITE_FLEET_HOST?: string;
  /** Development-time override pointing to a local relay (relay mode only). */
  readonly VITE_RELAY_URL?: string;
}

/** App version, injected by vite `define` from package.json (see vite.config.ts). */
declare const __APP_VERSION__: string;

/** Short git commit this bundle was built from, injected by vite `define` (see
 *  vite.config.ts `buildCommit`). Reported to the desktop in `client_hello` so
 *  it can flag a phone running a stale deploy. `"unknown"` when neither
 *  `VITE_APP_COMMIT` nor a git checkout was available at build time. */
declare const __APP_COMMIT__: string;
