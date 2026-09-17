// Dev-only test harness: suspend the **real** TerminalPane and connect a fake pty
// host behind it (emits 200 recognizable lines of text) so touch-scroll behavior can be
// verified in desktop Chrome's mobile emulation using patchwright to send real touch sequences —
// rather than guessing.
//
// This harness is not shipped in any build: scroll-harness.html exists only on the dev server;
// the vite build entry point is index.html.

import { createRoot } from "react-dom/client";
import TerminalPane from "./views/TerminalPane";
import type { FleetTransport } from "./transport";
import type { ProcOutputChunk, ProcRecord } from "./types";

const LINES = 200;
const LOG = Array.from({ length: LINES }, (_, i) => `line-${String(i).padStart(3, "0")}`).join(
  "\r\n",
);

const record: ProcRecord = {
  id: "harness",
  workspacePath: "/harness",
  command: "",
  status: "running",
  startedMs: Date.now(),
  cols: 80,
  rows: 24,
};

function b64(s: string): string {
  const bytes = new TextEncoder().encode(s);
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin);
}

/** Fake host. `?latency=800` can slow down responses to observe polling re-entrancy. */
const latency = Number(new URLSearchParams(location.search).get("latency") ?? "0");

const client = {
  connect() {},
  close() {},
  sayGoodbye() {},
  isAuthed: true,
  endpointLabel: "harness",
  async request<T>(method: string, params?: Record<string, unknown>): Promise<T> {
    if (latency) await new Promise((r) => setTimeout(r, latency));
    if (method === "proc_output") {
      const from = (params?.offset as number | undefined) ?? 0;
      const slice = LOG.slice(from);
      const chunk: ProcOutputChunk = {
        dataB64: b64(slice),
        nextOffset: from + slice.length,
        record,
      };
      return chunk as unknown as T;
    }
    return undefined as unknown as T;
  },
  answer: () => true,
} as unknown as FleetTransport;

// No probe needed to verify scroll position: read the first line's text from .xterm-rows
// directly — is it line-000 or line-1xx — that is the ground truth of what the user sees.

createRoot(document.getElementById("root")!).render(
  <TerminalPane
    client={client}
    proc={record}
    registerInput={() => {}}
    onRecord={() => {}}
  />,
);
