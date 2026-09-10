/**
 * Timing shim for `@tauri-apps/api/core` — the module every IPC call goes
 * through, ours and the plugin packages' alike.
 *
 * # Why this replaced wrapping `window.__TAURI_INTERNALS__.invoke`
 *
 * `invokeProbe.ts` tried to wrap the host's `invoke` in place. It cannot, and
 * never could: Tauri installs it as
 *
 *     Object.defineProperty(window.__TAURI_INTERNALS__, 'invoke', { value: … })
 *
 * (`tauri/scripts/core.js`), and a descriptor with a bare `value` defaults to
 * `writable: false, configurable: false`. So the property is neither
 * assignable nor redefinable. The 2026-09-08 "avoid wrapping readonly Tauri
 * invoke" fix made that case return quietly instead of throwing — which turned
 * the probe into a permanent no-op on the only host that matters. Proof: across
 * the entire 144 MB debug log, written by app builds that shipped the probe,
 * there is not one `[invoke]` line — including a run that sat on a >20s stall.
 *
 * Wrapping the *module* instead sidesteps the host entirely. Vite aliases
 * `@tauri-apps/api/core` to this file (see `vite.config.ts`), so the ~100 app
 * modules and the seven `@tauri-apps/plugin-*` packages — which import `invoke`
 * from the same specifier — all resolve here. That covers the plugin commands
 * (`plugin:dialog|save`, the native save panel that "did nothing") no
 * host-side wrapper reached either.
 *
 * The instrumentation itself is unchanged in spirit: log a round trip that is
 * slow when it lands, and — the load-bearing half — log one that is *still
 * outstanding*, because a promise that never settles never reaches the
 * completion branch, which is exactly how the original bug left no trace.
 */

import { invoke as realInvoke } from "@tauri-apps/api/core-real";

export * from "@tauri-apps/api/core-real";

/** Round trips at or above this are logged when they finish. */
const SLOW_MS = 1_000;

/**
 * ...and a call still outstanding at this point is logged *while* pending.
 */
const PENDING_MS = 3_000;

/** Cap on lines per window, so a long freeze cannot flood the log. */
const MAX_LINES = 20;
const WINDOW_MS = 10_000;

/** The command used to write the log — never timed, or it recurses. */
const LOG_CMD = "log_frontend_debug";

let windowStart = 0;
let linesInWindow = 0;

function report(line: string): void {
  const now = Date.now();
  if (now - windowStart > WINDOW_MS) {
    windowStart = now;
    linesInWindow = 0;
  }
  if (linesInWindow >= MAX_LINES) return;
  linesInWindow += 1;
  // Straight to the real invoke: a logged line must not be timed itself.
  void (realInvoke as (cmd: string, args?: unknown) => Promise<unknown>)(LOG_CMD, {
    msg: line,
  }).catch(() => {
    // Nowhere to report a failure to report. Dropping it is the whole budget
    // this probe is allowed to cost.
  });
}

/**
 * `invoke`, timed. Signature-compatible with the real one — it forwards every
 * argument untouched and returns the same promise, so a caller cannot tell the
 * difference except in the log.
 */
export function invoke<T>(
  cmd: string,
  args?: Record<string, unknown>,
  options?: unknown,
): Promise<T> {
  const call = () =>
    (realInvoke as (c: string, a?: unknown, o?: unknown) => Promise<T>)(cmd, args, options);
  if (cmd === LOG_CMD) return call();

  const started = Date.now();
  let settled = false;
  const pendingTimer = setTimeout(() => {
    if (!settled) report(`[invoke] ${cmd} still pending after ${PENDING_MS}ms`);
  }, PENDING_MS);
  const finish = (outcome: string) => {
    settled = true;
    clearTimeout(pendingTimer);
    const elapsed = Date.now() - started;
    if (elapsed >= SLOW_MS) {
      report(`[invoke] ${cmd} took ${elapsed}ms — ${outcome}`);
    }
  };

  return call().then(
    (value) => {
      finish("ok");
      return value;
    },
    (error: unknown) => {
      finish(`rejected: ${String(error).slice(0, 200)}`);
      throw error;
    },
  );
}
