// Terminal panel client: brings the proc_runner's pty host to phone/browser.
//
// The backend (claw-fleet-core/src/proc_runner.rs) is already a full interactive terminal
// — separate pty host, stdin forwarding, resize, group killpg, incremental output reads
// by offset. Desktop uses it via Backend trait; here we use the same proc_* methods via
// serve_request, so both ends see the same processes.
//
// Key property: **the pty host is separate**. Close this page, quit the browser, even
// switch devices — commands keep running on the desktop. So reopening the panel must
// `listProcs` to bring them back, not start a fresh shell each time.

import type { FleetTransport } from "./transport";
import type { ProcOutputChunk, ProcRecord } from "./types";

export type { ProcRecord, ProcOutputChunk, ProcStatus } from "./types";

/** All terminal processes on this host, newest first. Pass workspacePath to filter by workspace. */
export function listProcs(
  client: FleetTransport,
  workspacePath?: string,
): Promise<ProcRecord[]> {
  return client.request<ProcRecord[]>("procs", workspacePath ? { workspacePath } : {});
}

/** Open a new pty under workspacePath and run command. */
export function runProc(
  client: FleetTransport,
  workspacePath: string,
  command: string,
  cols: number,
  rows: number,
): Promise<ProcRecord> {
  return client.request<ProcRecord>("proc_run", { workspacePath, command, cols, rows });
}

/** Read output incrementally from offset; omit offset to mean "from the most recent chunk".
 *
 *  The returned chunk carries its record, so "did the command exit / what's the exit code"
 *  needs no second poll — desktop's ProcTerminal uses it the same way. */
export function readProcOutput(
  client: FleetTransport,
  id: string,
  offset: number | null,
): Promise<ProcOutputChunk> {
  return client.request<ProcOutputChunk>("proc_output", offset === null ? { id } : { id, offset });
}

/** Send keystrokes straight to the pty (raw base64 bytes, not text lines). */
export function writeProcInput(
  client: FleetTransport,
  id: string,
  dataB64: string,
): Promise<void> {
  return client.request<void>("proc_input", { id, dataB64 });
}

/** Tell the pty the new window size; full-screen programs like vim/htop redraw with it. */
export function resizeProc(
  client: FleetTransport,
  id: string,
  cols: number,
  rows: number,
): Promise<void> {
  return client.request<void>("proc_resize", { id, cols, rows });
}

/** Kill the command's entire process group. force skips SIGTERM grace and goes straight to SIGKILL. */
export function killProc(client: FleetTransport, id: string, force = false): Promise<void> {
  return client.request<void>("proc_kill", { id, force });
}

/** Delete an exited process's record and logs so reconnect lists don't bloat. */
export function clearProc(client: FleetTransport, id: string): Promise<{ cleared: number }> {
  return client.request<{ cleared: number }>("proc_clear", { id });
}

/** After exit, read a few more rounds: the host finishes writing `<id>.out` before flipping the
 *  record to exited, and missing those reads drops the last lines (often the actual error). */
export const EXIT_DRAIN_POLLS = 3;

export interface OutputPumpDeps {
  /** Read incremental output. offset null means "start following from the most recent". */
  read: (offset: number | null) => Promise<ProcOutputChunk>;
  /** Feed decoded bytes to the terminal. */
  write: (bytes: Uint8Array) => void;
  onRecord?: (record: ProcRecord) => void;
}

export interface OutputPump {
  /** Run one round of incremental read. Timer calls this once per tick. */
  poll: () => Promise<void>;
  /** Panel unmount: discard all subsequent responses, stop advancing offset. */
  stop: () => void;
}

/** Output pump: read pty output incrementally by offset and feed it to the terminal.
 *
 *  Extracted from TerminalPane for unit testing because it has a trap that only shows on
 *  slow links: `poll` is async, and offset only advances after await returns. The timer
 *  doesn't wait for the previous round to finish, so **when response is slower than the
 *  poll interval, two rounds send the same offset, and the same output gets written twice**
 *  — phone over relay to desktop is exactly that link type, and typing `ls` shows `llss`
 *  on screen (the pty still gets `ls`, so Enter still works). Desktop over local IPC is
 *  fast enough that it almost never happens. */
export function createOutputPump({ read, write, onRecord }: OutputPumpDeps): OutputPump {
  let offset: number | null = null;
  let drainPolls = 0;
  let stopped = false;
  // In-flight gate: don't send the next round until this one returns. The timer just
  // prods; the actual pace is set by link speed — on slow links polling naturally
  // degrades to "one question, one answer", not multiple rounds racing to fill the screen
  // with the same offset.
  let inFlight = false;

  return {
    async poll() {
      if (stopped || inFlight || drainPolls >= EXIT_DRAIN_POLLS) return;
      inFlight = true;
      try {
        const chunk = await read(offset);
        if (stopped) return;
        offset = chunk.nextOffset;
        if (chunk.dataB64) write(decodeOutput(chunk.dataB64));
        onRecord?.(chunk.record);
        if (chunk.record.status === "exited") drainPolls += 1;
      } catch {
        // Process record was cleared while the panel was open — stop advancing, don't
        // spam the error all over the screen.
        drainPolls = EXIT_DRAIN_POLLS;
      } finally {
        inFlight = false;
      }
    },
    stop() {
      stopped = true;
    },
  };
}

/** Encode xterm's onData string into the raw base64 bytes the backend expects.
 *
 *  Can't just `btoa(data)`: btoa only accepts latin1, and any non-ASCII input (Chinese,
 *  emoji, IME-committed whole phrases) throws InvalidCharacterError and silently loses
 *  the keystroke. */
export function encodeInput(data: string): string {
  const bytes = new TextEncoder().encode(data);
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin);
}

/** Decode base64 pty output back to bytes to feed xterm. */
export function decodeOutput(dataB64: string): Uint8Array {
  return Uint8Array.from(atob(dataB64), (c) => c.charCodeAt(0));
}

/** Fold a character into its control code: Ctrl-A..Z are 0x01..0x1a, plus a few symbols.
 *
 *  The soft keyboard has no Ctrl key, so a sticky button in the key row provides it,
 *  acting on the next character typed — this function does that step. Unrecognized
 *  characters pass through as-is: Ctrl has no definition for them, and dropping them
 *  would make it look like the keystroke was lost. */
export function applyCtrl(data: string): string {
  if (data.length !== 1) return data;
  const c = data.toUpperCase();
  if (c >= "A" && c <= "Z") return String.fromCharCode(c.charCodeAt(0) - 64);
  const punct: Record<string, string> = {
    "@": "\x00",
    " ": "\x00",
    "[": "\x1b",
    "\\": "\x1c",
    "]": "\x1d",
    "^": "\x1e",
    _: "\x1f",
  };
  return punct[c] ?? data;
}
