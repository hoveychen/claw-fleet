import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import type { ProcOutputChunk, ProcRecord } from "../types";

/** Post-exit drain polls: the host finishes writing `<id>.out` before it
 * flips the record to `exited`, so a couple of extra reads flush the tail. */
const EXIT_DRAIN_POLLS = 3;

/** xterm.js view attached to one workspace proc. Output arrives by polling
 * the backend's incremental log reads (works identically for Local and
 * Remote backends); keystrokes and resizes go back through the proc's
 * control socket. */
export function ProcTerminal({
  proc,
  onRecord,
  height = 320,
}: {
  proc: ProcRecord;
  /** Called with the record piggybacked on every output poll, so a caller that
   *  needs to know the proc exited (and with what code) doesn't have to run a
   *  second poll loop against the same log. */
  onRecord?: (record: ProcRecord) => void;
  /** Fixed pixel height (the 命令 panel's inline rows) or a CSS length — the
   *  终端 page passes `"100%"` to fill its pane. The ResizeObserver below
   *  re-fits either way, so a stretched terminal reflows with the window. */
  height?: number | string;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  // Held in a ref so a caller passing an inline closure doesn't tear down and
  // rebuild the terminal on every render.
  const onRecordRef = useRef(onRecord);
  onRecordRef.current = onRecord;

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;

    const term = new Terminal({
      fontSize: 12,
      fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
      theme: { background: "#16161e" },
      cursorBlink: true,
      scrollback: 5000,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(el);
    fit.fit();

    let disposed = false;
    let offset: number | null = null; // null = tail the recent output
    let drainPolls = 0;
    // In-flight gate: `offset` only advances once the await resolves, so if a
    // read takes longer than the poll interval the next tick goes out with the
    // *same* offset and the same pty echo gets written twice — typing `ls`
    // renders as `llss` while the pty only ever received `ls`. Local IPC is far
    // under 300ms, but RemoteBackend (SSH probe HTTP) is not. Same fix as
    // mobile-web's createOutputPump.
    let inFlight = false;

    const sendResize = () => {
      void invoke("resize_workspace_proc", {
        id: proc.id,
        cols: term.cols,
        rows: term.rows,
      }).catch(() => {
        // Exited proc has no control socket — resize is meaningless then.
      });
    };
    sendResize();

    const onData = term.onData((data) => {
      const bytes = new TextEncoder().encode(data);
      let bin = "";
      for (const b of bytes) bin += String.fromCharCode(b);
      void invoke("write_workspace_proc_input", {
        id: proc.id,
        dataB64: btoa(bin),
      }).catch(() => {});
    });

    const poll = async () => {
      if (disposed || inFlight || drainPolls >= EXIT_DRAIN_POLLS) return;
      inFlight = true;
      try {
        const chunk = await invoke<ProcOutputChunk>("read_workspace_proc_output", {
          id: proc.id,
          offset,
        });
        if (disposed) return;
        offset = chunk.nextOffset;
        if (chunk.dataB64) {
          const bytes = Uint8Array.from(atob(chunk.dataB64), (c) => c.charCodeAt(0));
          term.write(bytes);
        }
        onRecordRef.current?.(chunk.record);
        if (chunk.record.status === "exited") drainPolls += 1;
      } catch {
        // Proc was cleared while the terminal is open — stop advancing.
        drainPolls = EXIT_DRAIN_POLLS;
      } finally {
        inFlight = false;
      }
    };
    void poll();
    const timer = setInterval(() => void poll(), 300);

    const observer = new ResizeObserver(() => {
      fit.fit();
      sendResize();
    });
    observer.observe(el);

    return () => {
      disposed = true;
      clearInterval(timer);
      observer.disconnect();
      onData.dispose();
      term.dispose();
    };
  }, [proc.id]);

  return <div ref={containerRef} style={{ height, padding: "4px 0 0 6px" }} />;
}
