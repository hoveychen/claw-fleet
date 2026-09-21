// "Session Details" half-screen — the only metadata panel on the session detail page.
//
// It replaces three things:
//   1. The inline-expanded `infoPanel` below the header (fixed five static rows,
//      pushing content down ~90px, and taking up space even when empty);
//   2. The `SessionHeaderMenu` "Session Actions" sheet (copy id/path/resume command,
//      switch scope);
//   3. The six-tab bar below the header (Decisions/Plans/Tokens/Workflow/Handoff —
//      leaving ~46px per tab on a 390px-wide screen).
//
// These three were three entry points to three overlapping information sets: you had
// to click "Plans" twice to see where the plan is, but that number (`taskPlan.done/total`)
// already sat in the snapshot; "Switch scope" was in the ☰ menu, and "how many
// subagents are running" was missing from the header entirely. Combined into one
// half-screen, tapping the title or ☰ both lead here, and each row is both a
// readout and an entry point.
//
// Why a half-screen instead of continuing with inline expansion: inline panel height
// borrows from the content, so it must be small—it only fits five static rows. This
// is exactly what the boss said: "even expanded, won't fit watch and subagent." A
// half-screen borrows a **temporary** screen space, can use 85vh, so "what's
// happening now" finally has room to lay out. Handle + rounded corners + bottom
// safe area are the native mobile idiom for these "grab and go" panes.
//
// Desktop has no equivalent: desktop SessionDetail's header lays out a whole row of
// chips horizontally, and its tab bar has 1000+ px available. This half-screen is
// phone-specific consolidation.

import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import {
  Check,
  ChevronRight,
  Copy,
  FileJson2,
  Folder,
  Square,
  Terminal,
  Timer,
  X,
} from "lucide-react";
import { t } from "../i18n";
import { HistoryLayer } from "../useNavStack";
import type { SessionInfo } from "../types";
import type { FleetTransport } from "../transport";
import { useConfirm } from "../confirmDialog";
import { agentIdTail, agentLabel } from "./agentScope";
import { canControl, runStop, stopMode } from "./sessionStop";
import { buildInfoChips, resumeCommand } from "./sessionInfoRows";
import type { DetailPane } from "./sessionStatusPills";
import styles from "./SessionSheet.module.css";

/** A clickable row: name on the left, current readout on the right.
 *
 *  The readout is why this half-screen exists. The old tab bar had "Plans / Tokens /
 *  Workflow / Handoff" labels with no numbers, so you had to click each one to see
 *  which had content — four clicks to find out "oh, Workflow is empty." With readouts,
 *  you can scan most of them at a glance without clicking. */
interface PaneRow {
  pane: DetailPane;
  label: string;
  /** Readout on the right.
   *
   *  Three states, don't collapse to two:
   *  - A string value = snapshot tells us what's on this pane;
   *  - `"empty"` = snapshot **proves** this pane is empty (no taskPlan = no plans,
   *    no handoff = not in any handoff chain), display "none";
   *  - `undefined` = we don't know (Token and Workflow content is fetched on click,
   *    not in the snapshot), display nothing.
   *
   *  V1 treated the last two as null and displayed "none", so a $4.33 session
   *  showed "none" on the "Tokens & Cost" row — saying "I don't know" as "you have
   *  none" is the easiest and hardest-to-catch lie this half-screen can tell. */
  value?: string | "empty";
  /** Readout gets accent emphasis — only for "it's waiting on you" cases. */
  hot?: boolean;
  progress?: { done: number; total: number };
}

/** A text row to copy. Part of the same batch as the old ☰ menu items, following
 *  the same "await then show result" discipline: mobile browsers reject
 *  `clipboard.writeText` in non-secure contexts or without user gesture, so fire
 *  and forget results in "show copied, clipboard is empty." */
interface CopyRow {
  id: string;
  label: string;
  sub: string;
  icon: ReactNode;
  text: string;
}

export function SessionSheet({
  session,
  family,
  pendingDecisions,
  client,
  explainCount,
  onClose,
  onOpenPane,
  onOpenSession,
}: {
  session: SessionInfo;
  /** Side questions asked about this session so far. Not in the snapshot —
   *  the detail page reads the list over the relay and passes the count once
   *  it has it; `undefined` until then, which the row renders as silence. */
  explainCount?: number;
  /** Main process + all subagents (caller assembles, sorts, caps per desktop rules).
   *  Empty means this is a standalone session with no subagents; that section
   *  doesn't appear. */
  family: SessionInfo[];
  /** Count of pending decision cards for this session (decision cards are an
   *  aggregated cross-device inbox, not on SessionInfo, so caller counts and
   *  passes it). */
  pendingDecisions: number;
  /** Transport for **the device** this session belongs to — stop uses pid /
   *  workspacePath, so routing to the wrong device either fails to stop or kills
   *  an unrelated process. When `null` (device unreachable), this section
   *  disappears entirely: an unresponsive button is harder to explain than no
   *  button. */
  client: FleetTransport | null;
  onClose: () => void;
  onOpenPane: (pane: DetailPane) => void;
  onOpenSession: (s: SessionInfo) => void;
}) {
  const [result, setResult] = useState<{ id: string; ok: boolean } | null>(null);
  const clearTimer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(clearTimer.current), []);

  const copy = useCallback(async (row: CopyRow) => {
    window.clearTimeout(clearTimer.current);
    try {
      await navigator.clipboard.writeText(row.text);
      setResult({ id: row.id, ok: true });
      // ✓ Stay for a beat. Half-screen doesn't close after copy like the old menu —
      // it's a "dashboard," people often want to look at other rows afterward,
      // closing would require reopening.
      clearTimer.current = window.setTimeout(() => setResult(null), 1200);
    } catch {
      setResult({ id: row.id, ok: false });
    }
  }, []);

  // ── Stop / Interrupt ────────────────────────────────────────────────────────
  // Before this half-screen, there was no way to stop a session from the detail
  // page: you'd have to go back to the task list and find that card again. The
  // three states and confirmation text are shared with the list card in sessionStop.ts.
  const confirm = useConfirm();
  const [stopping, setStopping] = useState(false);
  const mode = stopMode(session);
  const stoppable = client !== null && canControl(session) && mode !== "spent";
  const doStop = useCallback(async () => {
    if (!client || stopping) return;
    setStopping(true);
    try {
      const done = await runStop(client, session, confirm);
      // If truly stopped, close the half-screen — leaving it open with static
      // readouts won't change, appearing ineffective.
      if (done) onClose();
    } catch (e) {
      window.alert(e instanceof Error ? e.message : t("操作失败"));
    } finally {
      setStopping(false);
    }
  }, [client, stopping, session, confirm, onClose]);

  const title = session.titleOverride || session.aiTitle || session.slug || t("会话");
  const resume = resumeCommand(session);

  // Static fields compressed into chip row (model / reasoning effort / workspace /
  // context / cost). Missing fields don't take space — rules and unit tests in
  // sessionInfoRows.ts.
  const chips = buildInfoChips(session);

  // ── Now ──────────────────────────────────────────────────────────────
  const nowRows: PaneRow[] = [];
  if (pendingDecisions > 0) {
    nowRows.push({
      pane: "decisions",
      label: t("待决策"),
      value: t("{0} 张", pendingDecisions),
      hot: true,
    });
  }
  const watches = session.watches ?? [];

  // ── Progress ─────────────────────────────────────────────────────────
  const progressRows: PaneRow[] = [];
  progressRows.push({
    pane: "plans",
    label: t("计划"),
    value: session.taskPlan
      ? session.taskPlan.currentTask
        ? `${session.taskPlan.currentTask} · ${session.taskPlan.done}/${session.taskPlan.total}`
        : `${session.taskPlan.done}/${session.taskPlan.total}`
      : "empty",
    progress: session.taskPlan ?? undefined,
  });
  // Token and Workflow content is not in the snapshot (fetched on click), so these
  // two rows don't get readouts — saying "none" would be lying about "I don't know."
  progressRows.push({ pane: "token", label: t("Token 与花费") });
  progressRows.push({ pane: "workflow", label: t("Workflow") });
  // Note count also not in snapshot, same reasoning — no readout.
  progressRows.push({ pane: "notes", label: t("笔记") });
  // Side questions: the count arrives with the detail page's own fetch, so it
  // is known here more often than not; before it lands, say nothing.
  progressRows.push({
    pane: "explains",
    label: t("追问"),
    value:
      explainCount === undefined ? undefined : explainCount === 0 ? "empty" : t("{0} 条", explainCount),
  });
  progressRows.push({
    pane: "handoff",
    label: t("接力链"),
    value: session.handoff
      ? t("第 {0} 棒 / 共 {1}", session.handoff.hop, session.handoff.chainLen)
      : "empty",
  });
  // Decision history must be accessible even with no pending cards — this pane
  // holds **answered** cards, and "which one did I click last time?" is the most
  // common question.
  if (pendingDecisions === 0) {
    progressRows.push({ pane: "decisions", label: t("决策记录") });
  }

  // ── Session ──────────────────────────────────────────────────────────
  const copyRows: CopyRow[] = [
    {
      id: "id",
      label: t("会话 ID"),
      sub: session.id,
      icon: <Copy size={15} />,
      text: session.id,
    },
    {
      id: "workspace",
      label: t("工作区路径"),
      sub: session.workspacePath,
      icon: <Folder size={15} />,
      text: session.workspacePath,
    },
    {
      id: "transcript",
      label: t("会话记录路径"),
      sub: session.jsonlPath,
      icon: <FileJson2 size={15} />,
      text: session.jsonlPath,
    },
  ];
  if (resume) {
    copyRows.push({
      id: "resume",
      label: t("恢复命令"),
      sub: resume,
      icon: <Terminal size={15} />,
      text: resume,
    });
  }

  const paneRow = (r: PaneRow) => (
    <button
      key={`${r.pane}-${r.label}`}
      type="button"
      className={styles.row}
      onClick={() => {
        onClose();
        onOpenPane(r.pane);
      }}
    >
      <span className={styles.rowLabel}>{r.label}</span>
      {r.progress && r.progress.total > 0 && (
        <span className={styles.bar} aria-hidden="true">
          <i style={{ width: `${Math.round((r.progress.done / r.progress.total) * 100)}%` }} />
        </span>
      )}
      {r.value === "empty" ? (
        <span className={styles.rowValue} data-empty="">
          {t("无")}
        </span>
      ) : (
        r.value !== undefined && (
          <span className={styles.rowValue} data-hot={r.hot || undefined}>
            {r.value}
          </span>
        )
      )}
      <ChevronRight size={15} className={styles.chev} />
    </button>
  );

  return (
    <>
      {/* Half-screen counts as a history layer: without it, back would close the
          whole session detail page. */}
      <HistoryLayer onBack={onClose} />
      {createPortal(
        <div className={styles.backdrop} onClick={onClose}>
          {/* z-index 55: must sit above the decision drawer's fixed bottom bar
              (DecisionDrawer is 45) — it's right where the half-screen expands.
              Learned from SessionHeaderMenu's testing. */}
          <div
            className={styles.sheet}
            role="dialog"
            aria-label={t("会话详情")}
            onClick={(e) => e.stopPropagation()}
          >
            <div className={styles.grabber} aria-hidden="true" />
            <button
              type="button"
              className={styles.close}
              onClick={onClose}
              aria-label={t("关闭")}
            >
              <X size={17} />
            </button>

            <div className={styles.title}>{title}</div>
            {chips.length > 0 && (
              <div className={styles.chips}>
                {chips.map((c) => (
                  <span key={c} className={styles.chip}>
                    {c}
                  </span>
                ))}
              </div>
            )}

            {/* Blocking errors in full. The pill row only fits "Remote Disconnected"
                or "Out of Credits" — why it failed, which host, and the full error
                all fit here. */}
            {session.outOfCredits && (
              <div className={styles.alert}>
                <span className={styles.alertLabel}>{t("额度耗尽")}</span>
                <span className={styles.alertBody}>{session.outOfCredits}</span>
              </div>
            )}
            {session.remoteDisconnect && (
              <div className={styles.alert}>
                <span className={styles.alertLabel}>
                  {t("远端断开")}
                  {session.remoteDisconnect.hostLabel
                    ? ` · ${session.remoteDisconnect.hostLabel}`
                    : ""}
                </span>
                <span className={styles.alertBody}>{session.remoteDisconnect.detail}</span>
              </div>
            )}
            {session.mirrorWrite && (
              <div className={styles.alert}>
                <span className={styles.alertLabel}>
                  {t("{0} 个文件留在本机", session.mirrorWrite.total)}
                </span>
                <span className={styles.alertBody}>
                  {session.mirrorWrite.files.slice(0, 4).join("\n")}
                </span>
              </div>
            )}

            {(nowRows.length > 0 || watches.length > 0 || family.length > 0 || stoppable) && (
              <div className={styles.section}>{t("此刻")}</div>
            )}
            {nowRows.map(paneRow)}

            {stoppable && (
              <button
                type="button"
                className={styles.stopRow}
                data-mode={mode}
                disabled={stopping}
                onClick={() => void doStop()}
              >
                <Square size={14} className={styles.stopIcon} />
                <span className={styles.stopText}>
                  <span className={styles.stopLabel}>
                    {mode === "interrupt" ? t("中断当前回合") : t("停止这个会话")}
                  </span>
                  {/* Big difference between the two, but the button labels can't explain
                      it: interrupt kills only the current turn, session stays and you can
                      send the next one; stop kills the process. */}
                  <span className={styles.stopSub}>
                    {mode === "interrupt"
                      ? t("只掐掉手上这一轮，会话还在，可以接着发下一条")
                      : t("结束这个进程，之后要用恢复命令才能继续")}
                  </span>
                </span>
                {stopping && <span className={styles.stopBusy}>…</span>}
              </button>
            )}

            {/* Watch doesn't have its own full page — its entire content is "what
                we're waiting for, how many polls, when to give up" — fits here,
                not worth a separate screen. */}
            {watches.map((w) => (
              <div key={w.id} className={styles.watchRow}>
                <Timer size={15} className={styles.watchIcon} />
                <span className={styles.watchText}>
                  <span className={styles.watchNote}>{w.note || t("未说明在等什么")}</span>
                  <span className={styles.watchMeta}>
                    {t("轮询 {0} 次 · 每 {1}s", w.pollCount, w.pollSecs)}
                  </span>
                </span>
              </div>
            ))}

            {/* Scope switching. The list from the old ☰ menu moved here as-is —
                it already belongs to "who's active in this session family right now,"
                sitting next to copy-path is just old menu baggage. */}
            {family.map((s) => {
              const isCurrent = s.id === session.id;
              return (
                <button
                  key={s.id}
                  type="button"
                  className={styles.row}
                  data-current={isCurrent ? "" : undefined}
                  onClick={() => {
                    onClose();
                    if (!isCurrent) onOpenSession(s);
                  }}
                >
                  <span className={styles.dot} data-status={s.status} />
                  <span className={styles.rowLabel}>
                    {agentLabel(s)}
                    {isCurrent && ` · ${t("当前")}`}
                  </span>
                  {s.isSubagent && (
                    <span className={styles.rowValue}>{agentIdTail(s.id)}</span>
                  )}
                  {!isCurrent && <ChevronRight size={15} className={styles.chev} />}
                </button>
              );
            })}

            <div className={styles.section}>{t("进度")}</div>
            {progressRows.map(paneRow)}

            <div className={styles.section}>{t("会话")}</div>
            {copyRows.map((row) => {
              const r = result?.id === row.id ? result : null;
              return (
                <button
                  key={row.id}
                  type="button"
                  className={styles.copyRow}
                  data-failed={r && !r.ok ? "" : undefined}
                  onClick={() => void copy(row)}
                >
                  <span className={styles.copyIcon}>
                    {r ? r.ok ? <Check size={15} /> : <X size={15} /> : row.icon}
                  </span>
                  <span className={styles.copyText}>
                    <span className={styles.copyLabel}>
                      {r && !r.ok ? t("复制失败（需要 HTTPS 或用户手势）") : row.label}
                    </span>
                    <span className={styles.copySub}>{row.sub}</span>
                  </span>
                </button>
              );
            })}
          </div>
        </div>,
        document.body,
      )}
    </>
  );
}
