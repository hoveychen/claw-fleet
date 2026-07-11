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
export function ProcTerminal({ proc }: { proc: ProcRecord }) {
  const containerRef = useRef<HTMLDivElement>(null);

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
      if (disposed || drainPolls >= EXIT_DRAIN_POLLS) return;
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
        if (chunk.record.status === "exited") drainPolls += 1;
      } catch {
        // Proc was cleared while the terminal is open — stop advancing.
        drainPolls = EXIT_DRAIN_POLLS;
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

  return <div ref={containerRef} style={{ height: 320, padding: "4px 0 0 6px" }} />;
}
