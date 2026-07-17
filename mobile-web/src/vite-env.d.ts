/// <reference types="vite/client" />

/** App version, injected by vite `define` from package.json (see vite.config.ts). */
declare const __APP_VERSION__: string;

/** Short git commit this bundle was built from, injected by vite `define` (see
 *  vite.config.ts `buildCommit`). Reported to the desktop in `client_hello` so
 *  it can flag a phone running a stale deploy. `"unknown"` when neither
 *  `VITE_APP_COMMIT` nor a git checkout was available at build time. */
declare const __APP_COMMIT__: string;
