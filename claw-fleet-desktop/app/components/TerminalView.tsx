// Terminal page: open a real shell for a repo, mirroring the mobile TerminalView.
//
// Division of labor with the "Repo → Commands" panel: that one **runs a command and leaves
// an execution record** (shortcuts, re-run, elapsed), this one is **I want a shell**. Both
// share the same proc registry, so a pty spawned on either side is visible from the other —
// intentional, one pty should have one source of truth.
//
// **Processes live on the backend host, not on this page.** The pty host is detached (see
// claw-fleet-core/src/proc_runner.rs); switching away from the page or restarting the app
// doesn't kill it. So the first thing on workspace switch is to reconnect live ones; only
// spawn new if there are none — otherwise every visit orphans another shell.

import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { FolderOpen, Plus, Square } from "lucide-react";
import { PageShell } from "./PageShell";
import { EmptyState } from "./EmptyState";
import { ProcTerminal } from "./ProcTerminal";
import { distinctWorkspaces } from "./NewSessionForm";
import { procLabel } from "./procCommandLabel";
import { isMissingProcError, terminalProcsForWorkspace } from "./terminalProcs";
import { useProcStore, useSessionsStore, useUIStore } from "../store";
import type { ProcRecord } from "../types";
import styles from "./MemoryView.module.css";
import termStyles from "./TerminalView.module.css";


export function TerminalView() {
  const { t } = useTranslation();
  const sessions = useSessionsStore((s) => s.sessions);
  const procs = useProcStore((s) => s.procs);
  const fetchProcs = useProcStore((s) => s.fetchProcs);
  const forgetProc = useProcStore((s) => s.forgetProc);

  const terminalNav = useUIStore((s) => s.terminalNav);
  const clearTerminalNav = useUIStore((s) => s.clearTerminalNav);

  const [selected, setSelected] = useState<string | null>(null);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // Spawn only once per repo automatically: reconnecting the list is async, without this gate,
  // "list empty → spawn one" would spawn again on the next render.
  const autoSpawned = useRef<string | null>(null);

  // List shares the same derivation as FilesView / new session: collapse worktrees back to repo root,
  // discard temporary scratchpad cwd. Limit at max — dropdowns need truncation, this is a full page.
  const workspaces = useMemo(
    () => distinctWorkspaces(sessions, Number.MAX_SAFE_INTEGER),
    [sessions],
  );

  // Workspace brought by "Open in terminal" from the files page. Depends on nonce not workspacePath —
  // when switching from terminal back to files and clicking the same repo, path doesn't change, only
  // nonce does, that's what makes this effect re-run.
  useEffect(() => {
    if (!terminalNav) return;
    setSelected(terminalNav.workspacePath);
    clearTerminalNav();
  }, [terminalNav?.nonce, terminalNav, clearTerminalNav]);

  // Poll the proc registry: tab liveness and pty spawned elsewhere (command panel, phone) all
  // come through here. ProcTerminal's own output polling only tracks its own.
  useEffect(() => {
    void fetchProcs();
    const timer = setInterval(() => void fetchProcs(), 2000);
    return () => clearInterval(timer);
  }, [fetchProcs]);

  const wsProcs = useMemo(
    () => terminalProcsForWorkspace(procs, selected),
    [procs, selected],
  );

  const spawn = useCallback(async () => {
    if (!selected || busy) return;
    setBusy(true);
    setError(null);
    try {
      // Empty command = let the **backend host** decide which interactive shell (`$SHELL -i` on unix,
      // `cmd` on Windows). Frontend guessing would guess wrong on remote backends. 80x24 is just
      // a start; ProcTerminal sends the real size immediately after mounting.
      const rec = await invoke<ProcRecord>("run_workspace_proc", {
        workspacePath: selected,
        command: "",
        cols: 80,
        rows: 24,
      });
      setActiveId(rec.id);
      void fetchProcs();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, [selected, busy, fetchProcs]);

  // Workspace switch → reconnect its existing terminals, only spawn new if none are live.
  useEffect(() => {
    if (!selected) return;
    let stale = false;
    setError(null);
    void (async () => {
      await fetchProcs();
      if (stale) return;
      const list = terminalProcsForWorkspace(useProcStore.getState().procs, selected);
      const live = list[0];
      setActiveId(live?.id ?? null);
      if (!live && autoSpawned.current !== selected) {
        autoSpawned.current = selected;
        void spawn();
      }
    })();
    return () => {
      stale = true;
    };
    // spawn depends on busy, every terminal open changes the reference; this effect should only
    // run on workspace change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selected]);

  const active = wsProcs.find((p) => p.id === activeId) ?? null;

  // The active shell can disappear after a poll (normal exit) or an explicit
  // not-found eviction. Keep another live shell selected instead of leaving a
  // row of tabs above an empty screen.
  useEffect(() => {
    if (activeId && wsProcs.some((proc) => proc.id === activeId)) return;
    setActiveId(wsProcs[0]?.id ?? null);
  }, [activeId, wsProcs]);

  const kill = async () => {
    if (!active) return;
    try {
      await invoke("kill_workspace_proc", { id: active.id, force: false });
    } catch (e) {
      if (isMissingProcError(e)) {
        forgetProc(active.id);
        setActiveId(null);
        setError(null);
        return;
      }
      setError(String(e));
    }
    void fetchProcs();
  };

  return (
    <PageShell
      view="terminal"
      title={t("terminal.title")}
      count={workspaces.length > 0 ? workspaces.length : null}
      secondary={
        <div className={styles.list_pane}>
          {workspaces.length === 0 && (
            <EmptyState
              icon={<FolderOpen size={28} strokeWidth={1.5} />}
              title={t("terminal.no_workspaces")}
            />
          )}
          <div className={styles.card_list}>
            {workspaces.map((ws) => (
              <button
                key={ws.path}
                className={`${styles.card} ${selected === ws.path ? styles.card_active : ""}`}
                onClick={() => setSelected(ws.path)}
              >
                <div className={styles.card_body}>
                  <div className={styles.card_title}>{ws.name}</div>
                  <div className={styles.card_hook}>{ws.path}</div>
                </div>
              </button>
            ))}
          </div>
        </div>
      }
    >
      {!selected ? (
        <div className={styles.placeholder}>{t("terminal.select_workspace")}</div>
      ) : (
        <div className={termStyles.pane}>
          <div className={termStyles.bar}>
            <div className={termStyles.tabs}>
              {wsProcs.map((p) => (
                <button
                  key={p.id}
                  className={termStyles.tab}
                  data-active={p.id === activeId}
                  onClick={() => setActiveId(p.id)}
                  title={p.command || t("terminal.shell")}
                >
                  {procLabel(p, t("terminal.shell"))}
                </button>
              ))}
            </div>
            <div className={termStyles.actions}>
              {active && (
                <button
                  className={termStyles.icon_btn}
                  onClick={() => void kill()}
                  title={t("terminal.kill")}
                >
                  <Square size={12} strokeWidth={1.8} />
                </button>
              )}
              <button
                className={termStyles.icon_btn}
                onClick={() => void spawn()}
                disabled={busy}
                title={t("terminal.new")}
              >
                <Plus size={14} strokeWidth={1.8} />
              </button>
            </div>
          </div>

          {error && <div className={termStyles.error}>{error}</div>}

          {active ? (
            // key = proc id: switching tabs must rebuild xterm, otherwise the new terminal keeps
            // writing to the previous one's buffer (ProcTerminal's effect is keyed by proc.id lifecycle).
            <div className={termStyles.screen}>
              <ProcTerminal
                key={active.id}
                proc={active}
                height="100%"
                onMissing={(id) => {
                  forgetProc(id);
                  setActiveId((current) => (current === id ? null : current));
                  setError(null);
                }}
              />
            </div>
          ) : (
            <div className={styles.placeholder}>
              {busy ? t("terminal.starting") : t("terminal.none")}
            </div>
          )}
        </div>
      )}
    </PageShell>
  );
}
