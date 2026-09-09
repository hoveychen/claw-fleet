/**
 * The browser build's transport for the environment panel's *install* actions.
 *
 * On the desktop, `invoke("install_harness")` is one Tauri command: it runs the
 * installer in-process and streams each output line on the
 * `harness-install-progress` app event, then resolves with the fresh
 * `HarnessStatus` (or rejects with a typed `InstallError`). There is no single
 * HTTP route with that shape, because an install takes minutes and a request
 * that hangs for minutes is not a route anyone can proxy.
 *
 * So `fleet serve` splits it the way it already splits a clone: `POST
 * /harness_install` spawns `fleet harness install <source>` as a proc and
 * returns its record; the client tails `/proc_output`. This module puts the
 * desktop's shape back together on top of that pair —
 *
 *   1. POST the action, get a `ProcRecord`
 *   2. poll `/proc_output` from a moving offset until the proc exits
 *   3. emit each *complete* new line as `harness-install-progress`, so the
 *      panel's existing `listen()` shows the same live log it does on desktop
 *   4. resolve / reject from the typed outcome on the marker line
 *
 * which is why `EnvironmentPanel` needs no branch for the browser: the promise
 * it awaits behaves the same on both clients.
 */

import { emit } from "@tauri-apps/api/event";

/**
 * Prefix of the machine-readable final output line. Must match
 * `fleet-cli/src/commands/harness.rs::RESULT_MARKER` — the two are one wire
 * contract, asserted from both sides.
 */
export const RESULT_MARKER = "__FLEET_HARNESS_RESULT__";

/** How often to ask for more output. Matches ProcTerminal's tail cadence. */
const POLL_MS = 500;

/**
 * Give up after this long with the proc still running.
 *
 * Not a substitute for the installer's own timeout (core's `run_streaming`
 * kills at `INSTALL_TIMEOUT`) — this is the client's guard against a proc host
 * that died without recording an exit, which would otherwise leave the panel's
 * button spinning forever.
 */
const MAX_WAIT_MS = 20 * 60 * 1000;

interface ProcRecord {
  id: string;
  status: "starting" | "running" | "exited";
  exitCode?: number | null;
}

interface ProcOutputChunk {
  dataB64: string;
  nextOffset: number;
  record: ProcRecord;
}

/** What the marker line carries: one of the two keys, never both. */
interface MarkerPayload {
  ok?: unknown;
  err?: { code: string; message: string };
}

/** The two calls this module needs from the proxy, injected so it is testable. */
export interface HarnessTransport {
  /** POST the action route; resolves with the spawned proc's record. */
  start: (path: string, body: Record<string, unknown>) => Promise<ProcRecord>;
  /** GET /proc_output at an offset. */
  output: (id: string, offset: number) => Promise<ProcOutputChunk>;
  /** Emit onto the app event bus. Defaults to Tauri's `emit`. */
  emitProgress?: (source: string, line: string) => void;
  /** Sleep, injected so tests do not actually wait. */
  sleep?: (ms: number) => Promise<void>;
  /** Clock, injected for the timeout test. */
  now?: () => number;
}

/**
 * Decode a base64 chunk as UTF-8.
 *
 * `atob` yields one char per *byte*, so a multi-byte character (an installer
 * printing 「验证中」, or the ellipsis in core's "verifying installation…")
 * would come out mojibake if the bytes were read as code units. Going through
 * TextDecoder is what keeps the log readable.
 */
function decodeChunk(b64: string): string {
  if (!b64) return "";
  const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
  return new TextDecoder("utf-8", { fatal: false }).decode(bytes);
}

/**
 * Split a growing pty stream into whole lines, keeping the incomplete tail.
 *
 * Two things a naive `split("\n")` gets wrong here. A pty ends lines with
 * `\r\n`, so every line would carry a trailing `\r` into the UI; and a chunk
 * boundary lands mid-line often (the poll is time-based, not line-based), so
 * emitting the last fragment would show half a line and then repeat it whole
 * on the next tick.
 */
export function splitLines(buffer: string): { lines: string[]; rest: string } {
  const parts = buffer.split("\n");
  const rest = parts.pop() ?? "";
  return { lines: parts.map((l) => l.replace(/\r$/, "")), rest };
}

/**
 * Pull the typed outcome out of the collected output.
 *
 * Scans backwards so a progress line that happens to echo the marker's name
 * (an installer printing the command it was given) cannot win over the real
 * final line.
 */
export function parseResult(lines: string[]): MarkerPayload | null {
  for (let i = lines.length - 1; i >= 0; i--) {
    const rest = lines[i].trimEnd();
    if (!rest.startsWith(RESULT_MARKER)) continue;
    try {
      return JSON.parse(rest.slice(RESULT_MARKER.length).trim()) as MarkerPayload;
    } catch {
      // A marker line we cannot parse is not a marker line — keep scanning
      // rather than reporting a corrupt success.
      continue;
    }
  }
  return null;
}

/**
 * Run one harness action to completion, streaming progress on the way.
 *
 * Resolves with the action's `ok` payload and rejects with its `err` — the
 * same contract the Tauri command has, including the `code` field the panel
 * branches on (`node-missing` is what makes it offer the Node bootstrap).
 *
 * `progressSource` is the key the panel files log lines under: the harness name
 * for install/update, the literal `"node"` for the Node bootstrap, exactly as
 * the desktop's emitters spell it.
 */
export async function runHarnessAction(
  t: HarnessTransport,
  path: string,
  body: Record<string, unknown>,
  progressSource: string,
): Promise<unknown> {
  const emitProgress =
    t.emitProgress ?? ((source: string, line: string) => void emit("harness-install-progress", { source, line }));
  const sleep = t.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));
  const now = t.now ?? (() => Date.now());

  const started = now();
  const rec = await t.start(path, body);

  let offset = 0;
  let pending = "";
  const lines: string[] = [];
  // One extra poll *after* the proc reports exited: the host writes the final
  // bytes and the exit status independently, so a loop that stops the moment
  // it sees `exited` can miss the marker line it is there to read.
  let drainsLeft = 1;
  // The *latest* record, not the one `start` returned: only a later poll knows
  // the exit code, and that code is the whole evidence in the no-marker branch.
  let last: ProcRecord = rec;

  for (;;) {
    const chunk = await t.output(rec.id, offset);
    offset = chunk.nextOffset;
    last = chunk.record;
    pending += decodeChunk(chunk.dataB64);
    const split = splitLines(pending);
    pending = split.rest;
    for (const line of split.lines) {
      lines.push(line);
      // The marker is plumbing, not progress — showing it would put a wall of
      // JSON in the panel's log tail as the last thing the user sees.
      if (!line.trimEnd().startsWith(RESULT_MARKER)) emitProgress(progressSource, line);
    }

    const exited = chunk.record.status === "exited";
    if (exited && drainsLeft <= 0) break;
    if (exited) drainsLeft -= 1;
    else if (now() - started > MAX_WAIT_MS) {
      throw {
        code: "timeout",
        message: `harness action did not finish within ${Math.round(MAX_WAIT_MS / 60000)} minutes`,
      };
    }
    await sleep(POLL_MS);
  }

  // A trailing fragment with no newline still matters — a CLI that dies right
  // after writing the marker may never emit its final `\n`.
  if (pending.trim()) lines.push(pending.replace(/\r$/, ""));

  const result = parseResult(lines);
  if (result?.err) throw result.err;
  if (result && "ok" in result) return result.ok;

  // No marker at all: the process died before reporting, so the exit code and
  // the tail of its output are the only evidence there is. Report *that*
  // rather than a bare "failed" — this is the branch a missing `fleet` binary
  // or a killed proc lands in.
  throw {
    code: "install-failed",
    message:
      `the install process ended without reporting a result` +
      (exitNote(last) ?? "") +
      (lines.length ? `\n${lines.slice(-8).join("\n")}` : ""),
  };
}

function exitNote(rec: ProcRecord): string | null {
  return rec.exitCode == null ? null : ` (exit ${rec.exitCode})`;
}
