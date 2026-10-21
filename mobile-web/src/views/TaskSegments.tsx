// The task detail page's segment switch: 对话 / 子任务 / 步骤 / 资料.
//
// 对话 is the transcript the page already shows; the other three are panes
// laid over it. 步骤 reuses TaskPlansTab as it is. 子任务 and 资料 live here.
// The developer panes (token, notes, workflow, handoff, …) stay behind the ⋮
// sheet — they describe the session, not the task.

import { useEffect, useMemo, useState } from "react";
import { ChevronRight, FolderOpen, GitFork, WifiOff } from "lucide-react";
import { t } from "../i18n";
import type { FleetTransport } from "../transport";
import type { Artifact, SessionInfo, SubtaskStep, TaskPlanDetail, WikiDoc } from "../types";
import { subtaskRowsOf, taskSessionIds, taskTitle } from "../taskRows";
import { repoRootPath } from "../../../shared-ts/repoPath";
import {
  fetchArtifactRecord,
  fetchTaskLibrary,
  fetchWikiDoc,
  type LibraryEntry,
  type TaskLibraryItems,
} from "../libraryApi";
import { EntryCards } from "./LibraryBrowse";
import { ArtifactDetail } from "./ArtifactsView";
import { Presence } from "../Presence";
import { isWakingSticky, statusTone, subtaskStateLabel, timeAgo } from "./TasksView";
import { subtaskOutcomeOnStep, type SubtaskOutcome } from "../../../shared-ts/subtaskOutcome";
import { EmptyState } from "./EmptyState";
import { SkeletonList } from "./loading";
import { SubtaskRecoveryBanner } from "./SubtaskRecovery";
import styles from "./TaskSegments.module.css";

export type TaskSegment = "chat" | "subtasks" | "steps" | "library";

const SEGMENT_LABEL: Record<TaskSegment, string> = {
  chat: "对话",
  subtasks: "子任务",
  steps: "步骤",
  library: "资料",
};

/** Which segments a session's page offers. A sub-task's own page has no
 *  子任务 segment — sub-tasks hang off the main conversation only. */
export function taskSegments(session: SessionInfo): TaskSegment[] {
  if (session.isSubagent) return [];
  return session.parentTaskId
    ? ["chat", "steps", "library"]
    : ["chat", "subtasks", "steps", "library"];
}

export function SegmentBar({
  segments,
  value,
  onChange,
  subtaskCount,
}: {
  segments: TaskSegment[];
  value: TaskSegment;
  onChange: (s: TaskSegment) => void;
  subtaskCount?: number;
}) {
  return (
    <div className={styles.bar} role="tablist">
      {segments.map((s) => (
        <button
          key={s}
          type="button"
          role="tab"
          aria-selected={value === s}
          className={styles.seg}
          data-active={value === s}
          onClick={() => onChange(s)}
        >
          {t(SEGMENT_LABEL[s])}
          {s === "subtasks" && !!subtaskCount && (
            <span className={styles.segCount}>{subtaskCount}</span>
          )}
        </button>
      ))}
    </div>
  );
}

export function useSubtasks(
  session: SessionInfo,
  sessions: SessionInfo[],
  pendingSessionIds?: ReadonlySet<string>,
) {
  return useMemo(
    () =>
      session.taskId && !session.parentTaskId
        ? subtaskRowsOf(sessions, session.taskId, {
            isPending: (s) => !!pendingSessionIds?.has(s.id),
            isQuiet: (s) => statusTone(s) === "quiet",
            isWaking: (s, pending) => isWakingSticky(s, pending),
          })
        : [],
    [session.taskId, session.parentTaskId, sessions, pendingSessionIds],
  );
}

type SubtaskRows = ReturnType<typeof useSubtasks>;

/** The step id (`P3`) a plan item's text starts with, if any. */
export function stepIdOf(text: string): string | null {
  return /^\s*\*{0,2}(P\d+[a-z]?)\*{0,2}/.exec(text)?.[1] ?? null;
}

/** Sub-task line id → the step it was dispatched for (`fleet__spawn` `step`),
 *  read off the task summary the main hops carry. */
export function subtaskStepLinks(session: SessionInfo): Map<string, SubtaskStep> {
  return new Map((session.taskSummary?.subtaskSteps ?? []).map((l) => [l.taskId, l]));
}

/** Key of one plan step, as `doneStepKeys` and the sub-task pane use it. */
function stepKey(planId: string, step: string): string {
  return `${planId}\u0000${step}`;
}

/** The checked-off steps across a task's plans, keyed by `stepKey`. */
export function doneStepKeys(plans: readonly TaskPlanDetail[]): Set<string> {
  const out = new Set<string>();
  for (const p of plans) {
    if (!p.id) continue;
    for (const item of p.items) {
      const step = item.done ? stepIdOf(item.text) : null;
      if (step) out.add(stepKey(p.id, step));
    }
  }
  return out;
}

/** The task's checked-off steps, fetched once per open of the sub-task pane.
 *  Empty until it lands or when the request fails: a failed sub-task then
 *  just keeps its red reading. */
function useDoneSteps(client: FleetTransport | null, workspacePath: string | null, sessionId: string | null) {
  const [done, setDone] = useState<Set<string>>(() => new Set());
  useEffect(() => {
    if (!client || !workspacePath || !sessionId) return;
    let cancelled = false;
    client
      .request<TaskPlanDetail[]>("task_plans", { workspacePath, sessionId })
      .then((plans) => {
        if (!cancelled) setDone(doneStepKeys(plans ?? []));
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [client, workspacePath, sessionId]);
  return done;
}

/** The sub-tasks dispatched for one step of `planId`. */
export function subtasksForStep(
  rows: SubtaskRows,
  links: Map<string, SubtaskStep>,
  planId: string | null | undefined,
  itemText: string,
): SubtaskRows {
  const step = stepIdOf(itemText);
  if (!step || !planId) return [];
  return rows.filter((r) => {
    const l = r.tip.taskId ? links.get(r.tip.taskId) : undefined;
    return l?.planId === planId && l.step === step;
  });
}

/** A step-linked sub-task's outcome; see `subtaskOutcomeOnStep`. `onStep` is
 *  every sub-task dispatched for the same step, this one included. */
function outcomeOnStep(r: SubtaskRows[number], stepDone: boolean, onStep: SubtaskRows): SubtaskOutcome | null {
  return subtaskOutcomeOnStep(
    { state: r.state, lastTurnEnd: r.tip.lastTurnEnd, createdAtMs: r.hops[0].createdAtMs },
    { done: stepDone, siblingCreatedAtMs: onStep.map((x) => x.hops[0].createdAtMs) },
  );
}

/** Compact sub-task rows nested under a step in the steps tab. */
export function StepSubtasks({
  rows,
  stepDone,
  onOpenSessionId,
}: {
  rows: SubtaskRows;
  /** The step these rows were dispatched for is checked off. */
  stepDone: boolean;
  onOpenSessionId: (id: string) => void;
}) {
  if (rows.length === 0) return null;
  return (
    <div className={`${styles.card} ${styles.stepSubtasks}`}>
      {rows.map((r) => {
        const outcome = outcomeOnStep(r, stepDone, rows);
        return (
          <button
            key={r.key}
            type="button"
            className={styles.row}
            onClick={() => onOpenSessionId(r.tip.id)}
          >
            <span className={styles.dot} data-state={r.state} data-outcome={outcome ?? undefined} />
            <span className={styles.rowBody}>
              <span className={styles.rowTitle}>{taskTitle(r.tip, t("未命名任务"))}</span>
              <span className={styles.rowMeta}>
                {r.pending ? t("等你回答") : subtaskStateLabel(r.state, outcome)}
              </span>
            </span>
            <ChevronRight size={16} className={styles.chevron} />
          </button>
        );
      })}
    </div>
  );
}

export function SubtasksPane({
  rows,
  links,
  onOpenSessionId,
  client = null,
  coordinatorId = null,
  workspacePath = null,
}: {
  rows: SubtaskRows;
  links?: Map<string, SubtaskStep>;
  onOpenSessionId: (id: string) => void;
  /** With both set, the pane offers 恢复中断的子任务 when a sub-task died
   *  mid-turn. */
  client?: FleetTransport | null;
  coordinatorId?: string | null;
  /** With `client` and `coordinatorId`, lets the pane read which plan steps
   *  are checked off, so a failed sub-task whose step is done reads as taken
   *  over. */
  workspacePath?: string | null;
}) {
  const doneSteps = useDoneSteps(client, workspacePath, coordinatorId);
  if (rows.length === 0) {
    return (
      <EmptyState
        compact
        icon={GitFork}
        title={t("还没有子任务")}
        description={t("这个任务派出的子任务会列在这里。")}
      />
    );
  }
  const titleOf = (id: string) => {
    const r = rows.find((x) => x.hops.some((h) => h.id === id));
    return r ? taskTitle(r.tip, t("未命名任务")) : null;
  };
  const keyOf = (r: SubtaskRows[number]) => {
    const l = r.tip.taskId ? links?.get(r.tip.taskId) : undefined;
    return l ? stepKey(l.planId, l.step) : null;
  };
  const outcomeOf = (r: SubtaskRows[number]) => {
    const key = keyOf(r);
    if (!key) return outcomeOnStep(r, false, [r]);
    return outcomeOnStep(r, doneSteps.has(key), rows.filter((x) => keyOf(x) === key));
  };
  return (
    <>
    <SubtaskRecoveryBanner client={client} coordinatorId={coordinatorId} titleOf={titleOf} />
    <div className={styles.card}>
      {rows.map((r) => (
        <button
          key={r.key}
          type="button"
          className={styles.row}
          onClick={() => onOpenSessionId(r.tip.id)}
        >
          <span className={styles.dot} data-state={r.state} data-outcome={outcomeOf(r) ?? undefined} />
          <span className={styles.rowBody}>
            <span className={styles.rowTitle}>{taskTitle(r.tip, t("未命名任务"))}</span>
            <span className={styles.rowMeta}>
              {r.pending ? t("等你回答") : subtaskStateLabel(r.state, outcomeOf(r))}
              {(() => {
                const step = r.tip.taskId ? links?.get(r.tip.taskId)?.step : undefined;
                return step ? ` · ${t("第 {0} 步", step.slice(1))}` : "";
              })()}
              {r.hops.length > 1 && ` · ${t("{0} 段", r.hops.length)}`}
            </span>
          </span>
          <span className={styles.rowTime}>{timeAgo(r.tip.lastActivityMs)}</span>
          <ChevronRight size={16} className={styles.chevron} />
        </button>
      ))}
    </div>
    </>
  );
}

export function TaskLibraryPane({
  session,
  sessions,
  client,
  onOpenDoc,
}: {
  session: SessionInfo;
  sessions: SessionInfo[];
  client: FleetTransport | null;
  onOpenDoc: (doc: WikiDoc) => void;
}) {
  // Keyed on the ids' content: `sessions` is a new array on every poll.
  const idsKey = useMemo(
    () => [...taskSessionIds(sessions, session)].sort().join("\n"),
    [sessions, session],
  );
  const [items, setItems] = useState<TaskLibraryItems | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [artifact, setArtifact] = useState<Artifact | null>(null);
  const [opening, setOpening] = useState(false);

  useEffect(() => {
    if (!client) return;
    let live = true;
    setError(null);
    fetchTaskLibrary(client, session.workspacePath, idsKey ? idsKey.split("\n") : [])
      .then((r) => live && setItems(r))
      .catch((e) => {
        if (!live) return;
        setError(e instanceof Error ? e.message : String(e));
        setItems((prev) => prev ?? { outputs: [], docs: [] });
      });
    return () => {
      live = false;
    };
  }, [client, session.workspacePath, idsKey]);

  // An item's record is fetched when it is tapped, as in the 资料 tab.
  const open = async (e: LibraryEntry) => {
    if (!client || opening) return;
    setOpening(true);
    setError(null);
    try {
      if (e.kind === "doc") onOpenDoc(await fetchWikiDoc(client, e.id));
      else setArtifact(await fetchArtifactRecord(client, e.id));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setOpening(false);
    }
  };

  if (!client) return <EmptyState compact icon={WifiOff} title={t("桌面端离线")} />;
  if (!items) return <SkeletonList rows={4} avatar />;
  const home = repoRootPath(session.workspacePath);
  return (
    <div className={styles.libraryPane} data-task-library>
      {error && <div className={styles.error}>{t("资料加载失败：{0}", error)}</div>}
      <div className={styles.groupHead}>{t("本任务产出")}</div>
      {items.outputs.length > 0 ? (
        <EntryCards client={client} entries={items.outputs} homeProject={home} onOpen={open} />
      ) : (
        <div className={styles.groupEmpty}>{t("这个任务还没有存进产出库的文件。")}</div>
      )}
      <div className={styles.groupHead}>
        <FolderOpen size={13} />
        {t("{0} 的文档", session.workspaceName || t("本项目"))}
      </div>
      {items.docs.length > 0 ? (
        <EntryCards client={client} entries={items.docs} homeProject={home} onOpen={open} />
      ) : (
        <div className={styles.groupEmpty}>{t("这个项目还没有归档的文档。")}</div>
      )}
      <Presence when={!!artifact}>
        {artifact && <ArtifactDetail artifact={artifact} client={client} onBack={() => setArtifact(null)} />}
      </Presence>
    </div>
  );
}
