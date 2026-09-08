/**
 * Time every invoke from the webview's side of the wire.
 *
 * # Why this exists, and why it is not `cmd_probe`
 *
 * The Rust-side `cmd_probe` times a command's own body, and only the seven
 * commands that were hand-wrapped with it. Neither half is enough for the
 * failure that ate two buttons and a save dialog on 2026-09-08:
 *
 *   - The stalled call was `artifact_local_path`, which had no probe. Adding
 *     one there — and to the next command that surprises us — is a list nobody
 *     keeps up to date; there are 250+ commands.
 *   - The command that *caused* the stall was fast by its own clock. It was
 *     holding the main thread, and the main thread is what carries every other
 *     command's *answer* back into the webview. A victim's Rust body may run in
 *     a microsecond and still leave its promise pending for seconds.
 *   - Plugin commands (`plugin:dialog|save`, the native save panel that "did
 *     nothing" when clicked) are not Fleet commands at all, so no amount of
 *     `cmd_probe` would ever have covered them.
 *
 * `@tauri-apps/api`'s `invoke()` reads `window.__TAURI_INTERNALS__.invoke` at
 * call time, so wrapping that one property measures the full round trip — args
 * out, answer back — for every call site in the app and every plugin, from
 * exactly the vantage point the user shares. Wiki:
 * `desktop/ipc-stall-forensics`.
 */

/** Round trips at or above this are logged when they finish. */
const SLOW_MS = 1_000;

/**
 * ...and a call still outstanding at this point is logged *while* pending.
 *
 * The load-bearing half: a promise that never settles never reaches the
 * completion branch, which is precisely how the original bug left no trace.
 */
const PENDING_MS = 3_000;

/** Cap on lines per window, so a long freeze cannot flood the log. */
const MAX_LINES = 20;
const WINDOW_MS = 10_000;

/** The command used to write the log — never wrapped, or it recurses. */
const LOG_CMD = "log_frontend_debug";

type RawInvoke = (cmd: string, args?: unknown, options?: unknown) => Promise<unknown>;

interface Internals {
  invoke?: RawInvoke;
  __fleetInvokeProbed?: boolean;
}

/**
 * Install the wrapper. Idempotent, and a no-op when there are no internals to
 * wrap (a plain browser before `installWebTransport`).
 *
 * Must run *after* whatever installs the internals — `installMocks()` or
 * `installWebTransport()` both replace the whole object, which would drop the
 * wrapper if it went first.
 */
export function installInvokeProbe(): void {
  if (typeof window === "undefined") return;
  const internals = (window as unknown as { __TAURI_INTERNALS__?: Internals })
    .__TAURI_INTERNALS__;
  const raw = internals?.invoke;
  if (!internals || typeof raw !== "function" || internals.__fleetInvokeProbed) return;
  internals.__fleetInvokeProbed = true;

  let windowStart = 0;
  let linesInWindow = 0;
  const report = (line: string) => {
    const now = Date.now();
    if (now - windowStart > WINDOW_MS) {
      windowStart = now;
      linesInWindow = 0;
    }
    if (linesInWindow >= MAX_LINES) return;
    linesInWindow += 1;
    // Through `raw`, not the wrapper: a logged line must not be timed itself.
    void raw.call(internals, LOG_CMD, { msg: line }).catch(() => {
      // Nowhere to report a failure to report. Dropping it is the whole
      // budget this probe is allowed to cost.
    });
  };

  internals.invoke = (cmd: string, args?: unknown, options?: unknown) => {
    if (cmd === LOG_CMD) return raw.call(internals, cmd, args, options);
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
    return raw.call(internals, cmd, args, options).then(
      (value) => {
        finish("ok");
        return value;
      },
      (error: unknown) => {
        finish(`rejected: ${String(error).slice(0, 200)}`);
        throw error;
      },
    );
  };
}
