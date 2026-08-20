import { invoke } from "@tauri-apps/api/core";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  ChevronDown,
  ChevronRight,
  Pin,
  PinOff,
  Play,
  RotateCw,
  Square,
  Trash2,
  X,
} from "lucide-react";
import { useProcStore } from "../store";
import { getItem, setItem } from "../storage";
import type { ProcRecord } from "../types";
import styles from "./FilesView.module.css";

const SHORTCUTS_KEY = "proc-shortcuts";

export interface ProcGroup {
  command: string;
  records: ProcRecord[];
  runningCount: number;
}

/** Collapse repeated commands while retaining every execution, newest first. */
export function groupProcs(procs: ProcRecord[]): ProcGroup[] {
  const groups = new Map<string, ProcGroup>();
  for (const proc of procs) {
    let group = groups.get(proc.command);
    if (!group) {
      group = { command: proc.command, records: [], runningCount: 0 };
      groups.set(proc.command, group);
    }
    group.records.push(proc);
    if (proc.status !== "exited") group.runningCount += 1;
  }
  return [...groups.values()];
}

/** Pinned commands, keyed by the absolute workspace path they were pinned in.
 *
 *  Pins MUST be per-repository: a shortcut is launched at the *current* panel's
 *  workspace cwd (see `launch`), so a globally-shared list would offer to run
 *  one repo's script inside an unrelated repo. */
export type ProcShortcutMap = Record<string, string[]>;

export function parseProcShortcuts(raw: string | null): ProcShortcutMap {
  if (!raw) return {};
  try {
    const value: unknown = JSON.parse(raw);
    // A bare array is the pre-per-workspace format: a flat global pin list with
    // no way to tell which repository each command belonged to. Drop it rather
    // than guess — a wrong guess re-creates the leak we are fixing.
    if (!value || typeof value !== "object" || Array.isArray(value)) return {};
    const map: ProcShortcutMap = {};
    for (const [workspace, commands] of Object.entries(value as Record<string, unknown>)) {
      if (!workspace.trim() || !Array.isArray(commands)) continue;
      const cleaned = [
        ...new Set(
          commands.filter((item): item is string => typeof item === "string" && !!item.trim()),
        ),
      ];
      if (cleaned.length > 0) map[workspace] = cleaned;
    }
    return map;
  } catch {
    return {};
  }
}

/** One repository's pins. The panel reads through this on every render, so
 *  switching repositories re-slices the same in-memory map — no remount and no
 *  effect needed to keep the list in sync with `workspace`. */
export function shortcutsFor(map: ProcShortcutMap, workspace: string): string[] {
  return map[workspace] ?? [];
}

/** The "命令" tab of a workspace's 仓库 detail pane: quick command launcher at
 * the workspace cwd + the list of procs (running and exited) hosted by detached
 * `fleet-proc-host` processes. */
export function ProcPanel({
  workspace,
  name,
  renderTerminal,
}: {
  workspace: string;
  name: string;
  /** Injected by FilesView once the terminal component exists — keeps the
   * panel testable without xterm. */
  renderTerminal?: (proc: ProcRecord) => React.ReactNode;
}) {
  const { t } = useTranslation();
  const procs = useProcStore((s) => s.procs);
  const fetchProcs = useProcStore((s) => s.fetchProcs);
  const [command, setCommand] = useState("");
  const [runError, setRunError] = useState<string | null>(null);
  const [openTermId, setOpenTermId] = useState<string | null>(null);
  const [openGroupCommand, setOpenGroupCommand] = useState<string | null>(null);
  const [shortcutMap, setShortcutMap] = useState<ProcShortcutMap>(() =>
    parseProcShortcuts(getItem(SHORTCUTS_KEY)),
  );
  const shortcuts = shortcutsFor(shortcutMap, workspace);

  const wsProcs = useMemo(
    () => procs.filter((p) => p.workspacePath === workspace),
    [procs, workspace],
  );
  const hasFinished = wsProcs.some((p) => p.status === "exited");
  const procGroups = useMemo(() => groupProcs(wsProcs), [wsProcs]);

  /** Replace *this* workspace's slot, leaving every other repository's pins
   *  untouched. An emptied slot is deleted so the map doesn't accumulate a key
   *  per repository the user ever visited. */
  const updateShortcuts = (next: string[]) => {
    const nextMap = { ...shortcutMap };
    if (next.length > 0) nextMap[workspace] = next;
    else delete nextMap[workspace];
    setShortcutMap(nextMap);
    setItem(SHORTCUTS_KEY, JSON.stringify(nextMap));
  };

  const toggleShortcut = (cmd: string) => {
    updateShortcuts(
      shortcuts.includes(cmd) ? shortcuts.filter((item) => item !== cmd) : [...shortcuts, cmd],
    );
  };

  /** Start `cmd` as a brand-new proc at the workspace cwd. Both the launcher
   * input and a row's 重跑 button land here — a re-run is just another launch. */
  const launch = async (cmd: string): Promise<boolean> => {
    if (!cmd) return false;
    setRunError(null);
    try {
      const rec = await invoke<ProcRecord>("run_workspace_proc", {
        workspacePath: workspace,
        command: cmd,
        cols: 80,
        rows: 24,
      });
      setOpenTermId(rec.id);
      void fetchProcs();
      return true;
    } catch (e) {
      setRunError(String(e));
      return false;
    }
  };

  const run = async () => {
    if (await launch(command.trim())) setCommand("");
  };

  const kill = async (id: string) => {
    try {
      await invoke("kill_workspace_proc", { id, force: false });
    } catch {
      // Row keeps showing running state; the next poll reconciles.
    }
    void fetchProcs();
  };

  const clear = async (id: string | null) => {
    try {
      await invoke("clear_workspace_procs", {
        id,
        workspacePath: id ? null : workspace,
      });
    } catch {
      // Clearing a proc that just restarted fails server-side; poll reconciles.
    }
    if (openTermId && (id === openTermId || id === null)) setOpenTermId(null);
    void fetchProcs();
  };

  return (
    <section className={styles.proc_panel}>
      {shortcuts.length > 0 && (
        <div className={styles.proc_shortcuts}>
          <div className={styles.proc_section_label}>{t("files.proc_shortcuts")}</div>
          <div className={styles.proc_shortcut_list}>
            {shortcuts.map((shortcut) => (
              <div className={styles.proc_shortcut} key={shortcut}>
                <button
                  className={styles.proc_shortcut_run}
                  onClick={() => void launch(shortcut)}
                  title={shortcut}
                >
                  <Play size={10} fill="currentColor" />
                  <code>{shortcut}</code>
                </button>
                <button
                  className={styles.proc_shortcut_remove}
                  onClick={() => toggleShortcut(shortcut)}
                  title={t("files.proc_shortcut_remove")}
                  aria-label={t("files.proc_shortcut_remove")}
                >
                  <X size={11} />
                </button>
              </div>
            ))}
          </div>
        </div>
      )}

      {hasFinished && (
        <div className={styles.proc_header}>
          <span className={styles.proc_spacer} />
          <button className={styles.proc_action} onClick={() => void clear(null)}>
            <Trash2 size={12} strokeWidth={1.5} />
            {t("files.proc_clear_finished")}
          </button>
        </div>
      )}

      <form
        className={styles.proc_input_row}
        onSubmit={(e) => {
          e.preventDefault();
          void run();
        }}
      >
        <input
          className={styles.proc_input}
          value={command}
          onChange={(e) => setCommand(e.target.value)}
          placeholder={t("files.proc_input_placeholder", { name })}
          spellCheck={false}
        />
        <button className={styles.proc_run_btn} type="submit" disabled={!command.trim()}>
          <Play size={12} strokeWidth={2} />
          {t("files.proc_run")}
        </button>
      </form>
      {runError && (
        <div className={styles.proc_error}>{t("files.proc_run_error", { error: runError })}</div>
      )}

      {wsProcs.length === 0 && <p className={styles.proc_empty}>{t("files.proc_empty")}</p>}

      <div className={styles.proc_list}>
        {procGroups.map((group) => {
          const groupOpen = openGroupCommand === group.command;
          const pinned = shortcuts.includes(group.command);
          const latest = group.records[0];
          return (
            <div key={group.command} className={styles.proc_group}>
              <div className={styles.proc_group_row}>
                <button
                  className={styles.proc_row_main}
                  onClick={() => setOpenGroupCommand(groupOpen ? null : group.command)}
                  title={group.command}
                >
                  {groupOpen ? (
                    <ChevronDown size={12} strokeWidth={1.5} />
                  ) : (
                    <ChevronRight size={12} strokeWidth={1.5} />
                  )}
                  <span
                    className={`${styles.proc_dot} ${
                      group.runningCount > 0 ? styles.proc_dot_running : styles.proc_dot_exited
                    }`}
                  />
                  <code className={styles.proc_command}>{group.command}</code>
                  <span className={styles.proc_status}>
                    {group.records.length > 1 &&
                      `${t("files.proc_run_count", { count: group.records.length })} · `}
                    {group.runningCount > 0
                      ? t("files.proc_running_count", { count: group.runningCount })
                      : statusLabel(latest, t)}
                  </span>
                </button>
                <button
                  className={styles.proc_action}
                  onClick={() => toggleShortcut(group.command)}
                  title={
                    pinned
                      ? t("files.proc_shortcut_remove")
                      : t("files.proc_shortcut_add")
                  }
                >
                  {pinned ? (
                    <PinOff size={11} strokeWidth={1.5} />
                  ) : (
                    <Pin size={11} strokeWidth={1.5} />
                  )}
                  {t("files.proc_shortcut")}
                </button>
                <button className={styles.proc_action} onClick={() => void launch(group.command)}>
                  <RotateCw size={11} strokeWidth={1.5} />
                  {t("files.proc_rerun")}
                </button>
              </div>
              {groupOpen && (
                <div className={styles.proc_group_runs}>
                  {group.records.map((p) => {
                    const termOpen = openTermId === p.id;
                    const running = p.status !== "exited";
                    return (
                      <div key={p.id} className={styles.proc_item}>
                        <div className={styles.proc_row}>
                          <button
                            className={styles.proc_row_main}
                            onClick={() => setOpenTermId(termOpen ? null : p.id)}
                            title={p.command}
                          >
                            {termOpen ? (
                              <ChevronDown size={12} strokeWidth={1.5} />
                            ) : (
                              <ChevronRight size={12} strokeWidth={1.5} />
                            )}
                            <span
                              className={`${styles.proc_dot} ${
                                running ? styles.proc_dot_running : styles.proc_dot_exited
                              }`}
                            />
                            <span className={styles.proc_execution_label}>
                              {t("files.proc_execution")}
                            </span>
                            <span className={styles.proc_status}>{statusLabel(p, t)}</span>
                          </button>
                          {running ? (
                            <button
                              className={`${styles.proc_action} ${styles.proc_action_danger}`}
                              onClick={() => void kill(p.id)}
                            >
                              <Square size={11} strokeWidth={2} />
                              {t("files.proc_kill")}
                            </button>
                          ) : (
                            <button className={styles.proc_action} onClick={() => void clear(p.id)}>
                              <Trash2 size={11} strokeWidth={1.5} />
                              {t("files.proc_clear")}
                            </button>
                          )}
                        </div>
                        {termOpen && renderTerminal && (
                          <div className={styles.proc_term_wrap}>{renderTerminal(p)}</div>
                        )}
                      </div>
                    );
                  })}
                </div>
              )}
            </div>
          );
        })}
      </div>
    </section>
  );
}

function statusLabel(p: ProcRecord, t: (k: string, o?: Record<string, unknown>) => string): string {
  if (p.status === "starting") return t("files.proc_starting");
  if (p.status === "running") return `${t("files.proc_running")} · ${fmtDuration(Date.now() - p.startedMs)}`;
  const exited =
    p.exitCode !== undefined
      ? t("files.proc_exited", { code: p.exitCode })
      : t("files.proc_exited_unknown");
  // Exited rows gain "took Xs · Y ago": duration = finish − start, "ago" anchored
  // to when the command was launched. finishedMs is absent on inferred exits.
  const parts = [exited];
  if (p.finishedMs != null) {
    parts.push(t("files.proc_took", { d: fmtDuration(p.finishedMs - p.startedMs) }));
  }
  parts.push(timeAgo(p.startedMs, t));
  return parts.join(" · ");
}

/** Format a millisecond span as a single-line, whole-second duration. */
function fmtDuration(ms: number): string {
  const secs = Math.max(0, Math.floor(ms / 1000));
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m${secs % 60 > 0 ? ` ${secs % 60}s` : ""}`;
  return `${Math.floor(secs / 3600)}h ${Math.floor((secs % 3600) / 60)}m`;
}

/** How long ago `ms` was, reusing the app-wide relative-time keys. */
function timeAgo(ms: number, t: (k: string, o?: Record<string, unknown>) => string): string {
  const diff = Date.now() - ms;
  if (diff < 60_000) return t("just_now");
  if (diff < 3_600_000) return t("m_ago", { n: Math.floor(diff / 60_000) });
  if (diff < 86_400_000) return t("h_ago", { n: Math.floor(diff / 3_600_000) });
  return t("d_ago", { n: Math.floor(diff / 86_400_000) });
}
