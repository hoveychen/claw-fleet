// Terminal page: full-page overlay entered via "Terminal" in task page top bar.
//
// A workspace can have multiple ptys open; top tabs switch between them. **Processes
// run on the desktop machine, not this page**—pty host is separate, and closing the
// page, quitting the browser, or switching devices leaves commands running. So first
// thing: `listProcs` reconnects to live ones; only spawn new if none exist. Otherwise
// each panel open adds an orphan shell.

import { lazy, Suspense, useCallback, useEffect, useRef, useState } from "react";
import { Folder, Plus, Square, Trash2 } from "lucide-react";
import { EmptyState } from "./EmptyState";
import { t } from "../i18n";
import type { FleetTransport } from "../transport";
import { clearProc, killProc, listProcs, runProc, type ProcRecord } from "../terminal";
import { isDefaultShellCommand } from "../../../shared-ts/procShell";
import styles from "./TerminalView.module.css";
import { AppHeader } from "./AppHeader";

// xterm and its CSS only download when a terminal actually opens — see the note at
// the top of TerminalPane.
const TerminalPane = lazy(() => import("./TerminalPane"));

/** A workspace that can open a terminal. deviceId determines which host this command
 *  runs on — with multiple devices, two machines can have workspaces with the same
 *  name or path; using only the path opens on the wrong machine. */
export interface TerminalWorkspace {
  deviceId: string;
  path: string;
  name: string;
}

interface Props {
  /** Available workspaces (taken from those appearing in the task list). */
  workspaces: TerminalWorkspace[];
  /** When task page has pre-filtered a directory, pass it to skip one selection. */
  initial?: TerminalWorkspace | null;
  clientFor: (deviceId: string) => FleetTransport | null;
  onBack: () => void;
}

/** Key bar: keys missing from soft keyboard. Sends full escape sequences, no longer
 *  folded through sticky Ctrl. */
const KEYS: Array<{ label: string; data: string }> = [
  { label: "Esc", data: "\x1b" },
  { label: "Tab", data: "\t" },
  { label: "^C", data: "\x03" },
  { label: "^D", data: "\x04" },
  { label: "↑", data: "\x1b[A" },
  { label: "↓", data: "\x1b[B" },
  { label: "←", data: "\x1b[D" },
  { label: "→", data: "\x1b[C" },
];

/** Height occupied by soft keyboard.
 *
 *  On iOS, soft keyboard doesn't shrink layout viewport—it overlays. This page is
 *  `position:fixed; inset:0`, so key bar and terminal bottom get fully covered; the
 *  typing line becomes invisible. visualViewport is the only API that tells "how much
 *  viewport height actually remains". */
function useKeyboardInset(): number {
  const [inset, setInset] = useState(0);
  useEffect(() => {
    const vv = window.visualViewport;
    if (!vv) return; // Older browsers: fall back to layout viewport, which shrinks
                     // naturally when keyboard pops (most Android behavior)
    const apply = () => {
      setInset(Math.max(0, window.innerHeight - vv.height - vv.offsetTop));
    };
    apply();
    vv.addEventListener("resize", apply);
    vv.addEventListener("scroll", apply);
    return () => {
      vv.removeEventListener("resize", apply);
      vv.removeEventListener("scroll", apply);
    };
  }, []);
  return inset;
}

/** Command name shown on tab: default shell displays as "shell", others take first word,
 *  truncate if too long.
 *
 *  "Is this the default shell?" check is shared with desktop (shared-ts/procShell.ts).
 *  We once had separate regexes; after core changed default command from `$SHELL` to
 *  absolute path, only one was synced, so the other showed default shell as "exec".
 *  Truncation length is mobile-specific (14 chars vs desktop's 16). */
function procLabel(proc: ProcRecord): string {
  const cmd = proc.command.trim();
  if (isDefaultShellCommand(cmd)) return t("shell");
  const head = cmd.split(/\s+/)[0] ?? cmd;
  return head.length > 14 ? `${head.slice(0, 13)}…` : head;
}

export function TerminalView({ workspaces, initial, clientFor, onBack }: Props) {
  const [ws, setWs] = useState<TerminalWorkspace | null>(initial ?? null);
  const [procs, setProcs] = useState<ProcRecord[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // Auto-spawn shell at most once per workspace: fetching the list is async, and without
  // this gate, "list empty → spawn one" would spawn again on second render.
  const autoSpawned = useRef<string | null>(null);
  // Key bar → current terminal's channel. TerminalPane passes send on mount, null on
  // unmount.
  const sendRef = useRef<((data: string) => void) | null>(null);
  const [ctrl, setCtrl] = useState(false);
  const keyboardInset = useKeyboardInset();

  const client = ws ? clientFor(ws.deviceId) : null;

  const spawn = useCallback(async () => {
    if (!client || !ws || busy) return;
    setBusy(true);
    setError(null);
    try {
      // Empty command = let host choose which interactive shell (unix `$SHELL -i`,
      // Windows `cmd`). 80x24 is just a starting point; panel sends real size
      // immediately on mount.
      const rec = await runProc(client, ws.path, "", 80, 24);
      setProcs((prev) => [rec, ...prev]);
      setActiveId(rec.id);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }, [client, ws, busy]);

  // Switch workspace → reconnect its existing terminals.
  useEffect(() => {
    if (!client || !ws) return;
    let stale = false;
    setError(null);
    void (async () => {
      try {
        const list = await listProcs(client, ws.path);
        if (stale) return;
        setProcs(list);
        const live = list.find((p) => p.status !== "exited");
        setActiveId(live?.id ?? list[0]?.id ?? null);
        if (!live && autoSpawned.current !== ws.path) {
          autoSpawned.current = ws.path;
          void spawn();
        }
      } catch (e) {
        if (!stale) setError(e instanceof Error ? e.message : String(e));
      }
    })();
    return () => {
      stale = true;
    };
    // spawn depends on busy, reference changes each terminal spawn; this effect should
    // only run on workspace/device change, so intentionally omit it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, ws?.path]);

  const active = procs.find((p) => p.id === activeId) ?? null;

  /** Output polling incidentally brings back latest record—use it to update tab alive
   *  state. */
  const onRecord = useCallback((rec: ProcRecord) => {
    setProcs((prev) => prev.map((p) => (p.id === rec.id ? rec : p)));
  }, []);

  const handleKill = useCallback(async () => {
    if (!client || !active) return;
    try {
      await killProc(client, active.id);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [client, active]);

  /** Close an exited terminal: delete record + remove from tabs. */
  const handleClear = useCallback(async () => {
    if (!client || !active) return;
    try {
      await clearProc(client, active.id);
      setProcs((prev) => {
        const rest = prev.filter((p) => p.id !== active.id);
        setActiveId(rest[0]?.id ?? null);
        return rest;
      });
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [client, active]);

  // ── No workspace selected yet: choose first ──────────────────────────────
  if (!ws) {
    return (
      <div className={styles.page}>
        <AppHeader onBack={onBack} title={t("终端")} />
        {workspaces.length === 0 ? (
          <EmptyState icon={Folder} title={t("还没有可用的工作目录")} />
        ) : (
          <div className={styles.pickList}>
            {workspaces.map((w) => (
              <button
                key={`${w.deviceId}::${w.path}`}
                className={styles.pickRow}
                onClick={() => setWs(w)}
              >
                <Folder size={15} />
                <span className={styles.pickName}>{w.name}</span>
                <span className={styles.pickPath}>{w.path}</span>
              </button>
            ))}
          </div>
        )}
      </div>
    );
  }

  const exited = active?.status === "exited";

  return (
    <div className={styles.page} style={{ bottom: keyboardInset }}>
      {/* Title doubles as the back button to workspace picker, so it stays a node
          not a string. `seamless` tracks the tab strip below: with multiple
          processes, header and .tabs form one panel. */}
      <AppHeader
        onBack={onBack}
        seamless={procs.length > 1}
        title={
          <button className={styles.headerTitle} onClick={() => setWs(null)} title={ws.path}>
            {ws.name}
          </button>
        }
        actions={
          <>
            {exited ? (
              <button className={styles.iconButton} onClick={() => void handleClear()}>
                <Trash2 size={16} />
              </button>
            ) : (
              active && (
                <button className={styles.iconButton} onClick={() => void handleKill()}>
                  <Square size={14} />
                </button>
              )
            )}
            <button
              className={styles.iconButton}
              onClick={() => void spawn()}
              disabled={busy}
              aria-label={t("新终端")}
            >
              <Plus size={18} />
            </button>
          </>
        }
      />

      {procs.length > 1 && (
        <div className={styles.tabs}>
          {procs.map((p) => (
            <button
              key={p.id}
              className={styles.tab}
              data-active={p.id === activeId}
              data-exited={p.status === "exited"}
              onClick={() => setActiveId(p.id)}
            >
              {procLabel(p)}
            </button>
          ))}
        </div>
      )}

      {error && <div className={styles.error}>{error}</div>}

      {client && active ? (
        <Suspense fallback={<EmptyState compact icon={Folder} title={t("加载中…")} />}>
          <TerminalPane
            key={active.id}
            client={client}
            proc={active}
            onRecord={onRecord}
            registerInput={(send) => {
              sendRef.current = send;
            }}
            ctrl={ctrl}
            onCtrlConsumed={() => setCtrl(false)}
          />
        </Suspense>
      ) : (
        <EmptyState compact icon={Folder} title={busy ? t("正在开终端…") : t("没有终端")} />
      )}

      {!exited && active && (
        <div className={styles.keyBar}>
          <button
            className={styles.key}
            data-sticky={ctrl}
            // onPointerDown + preventDefault: fire on press, and prevent the browser
            // from moving focus away from xterm's hidden textarea — losing focus makes
            // the soft keyboard close, and the key bar is meant to be used with it.
            onPointerDown={(e) => {
              e.preventDefault();
              setCtrl((v) => !v);
            }}
          >
            Ctrl
          </button>
          {KEYS.map((k) => (
            <button
              key={k.label}
              className={styles.key}
              onPointerDown={(e) => {
                e.preventDefault();
                sendRef.current?.(k.data);
              }}
            >
              {k.label}
            </button>
          ))}
        </div>
      )}

      {exited && active && (
        <div className={styles.exitBar}>
          {active.exitCode === null || active.exitCode === undefined
            ? t("已退出")
            : t("已退出 · 退出码 {0}", String(active.exitCode))}
          <button className={styles.exitAction} onClick={() => void spawn()}>
            {t("重开")}
          </button>
        </div>
      )}
    </div>
  );
}
