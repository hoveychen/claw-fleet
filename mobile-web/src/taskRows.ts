// Folding the session snapshot into the phone's task list.
//
// A task is a main conversation (every relay hop of it) plus the sub-tasks it
// spawned. The snapshot is flat — one row per session — so the list first
// regroups it: main hops by `taskId`, sub-task hops by `parentTaskId`. The
// task's section comes from `shared-ts/taskState.ts`, the same derivation the
// desktop home uses, so both clients file a task under the same heading.

import { repoRootPath } from "../../shared-ts/repoPath";
import {
  TASK_STATES,
  taskStateOf,
  type TaskMemberView,
  type TaskState,
} from "../../shared-ts/taskState";
import { isFleetOwnedTask, type SessionInfo } from "./types";

type Row = SessionInfo & { deviceId: string };

export interface SubtaskRow<T extends SessionInfo = Row> {
  key: string;
  /** The hop to open: the sub-task's latest. */
  tip: T;
  hops: T[];
  state: TaskState;
  /** A card from this sub-task is waiting for an answer. */
  pending: boolean;
}

export interface TaskRow<T extends Row = Row> {
  /** `deviceId::taskKey` — ids are unique per machine only. */
  key: string;
  deviceId: string;
  /** The main-conversation hop to open: the latest one. */
  tip: T;
  hops: T[];
  subtasks: Array<SubtaskRow<T>>;
  state: TaskState;
  title: string;
  /** Project scope key — same encoding as the task page's project menu. */
  projectKey: string;
  projectName: string;
  /** A card from the main conversation is waiting. */
  mainPending: boolean;
  lastActivityMs: number;
}

/** Hop order inside one conversation line: the relay hop number when there
 *  is one, then recency. */
function laterHop(a: SessionInfo, b: SessionInfo): boolean {
  const ha = a.handoff?.hop ?? 0;
  const hb = b.handoff?.hop ?? 0;
  if (ha !== hb) return ha > hb;
  return a.lastActivityMs > b.lastActivityMs;
}

function tipOf<T extends SessionInfo>(hops: T[]): T {
  return hops.reduce((a, b) => (laterHop(b, a) ? b : a));
}

/** Which conversation line a session belongs to. `taskId` is the core's
 *  answer; payloads from an older desktop lack it, and then a relay chain is
 *  the best stand-in for "one line". */
function lineKey(s: SessionInfo): string {
  if (s.taskId) return s.taskId;
  if (s.handoff && s.handoff.chainLen > 1) return `chain:${s.handoff.chainId}`;
  return s.id;
}

/** Value of a project scope option. Single-device: the path itself. */
export function projectScopeKey(
  deviceId: string | undefined,
  workspacePath: string,
  multiDevice: boolean,
): string {
  return multiDevice && deviceId ? `${deviceId}::${workspacePath}` : workspacePath;
}

export function taskTitle(s: SessionInfo, fallback: string): string {
  return s.titleOverride || s.aiTitle || s.slug || fallback;
}

export interface BuildOptions {
  isPending: (deviceId: string, sessionId: string) => boolean;
  /** The caller's latched quiet-alive verdict (see TasksView `statusTone`). */
  isQuiet: (s: Row) => boolean;
  multiDevice: boolean;
  deviceLabelOf?: (deviceId: string) => string | null | undefined;
  untitled: string;
}

/**
 * Regroup a device-tagged session snapshot into task rows, newest activity
 * first. Only Fleet-started main conversations become rows; a sub-task hop
 * joins the task named by its `parentTaskId` on the same device.
 */
export function buildTaskRows<T extends Row>(sessions: T[], opts: BuildOptions): Array<TaskRow<T>> {
  const mains = new Map<string, T[]>();
  const subs = new Map<string, Map<string, T[]>>();
  for (const s of sessions) {
    if (s.isSubagent) continue;
    if (s.parentTaskId) {
      const parent = `${s.deviceId}::${s.parentTaskId}`;
      let lines = subs.get(parent);
      if (!lines) subs.set(parent, (lines = new Map()));
      const k = lineKey(s);
      const arr = lines.get(k);
      if (arr) arr.push(s);
      else lines.set(k, [s]);
      continue;
    }
    if (!isFleetOwnedTask(s)) continue;
    const k = `${s.deviceId}::${lineKey(s)}`;
    const arr = mains.get(k);
    if (arr) arr.push(s);
    else mains.set(k, [s]);
  }

  const view = (s: T): TaskMemberView => ({
    status: s.status,
    procAlive: !!s.procAlive,
    quiet: opts.isQuiet(s),
    hasPendingDecision: opts.isPending(s.deviceId, s.id),
  });

  const rows: Array<TaskRow<T>> = [];
  for (const [key, hops] of mains) {
    const tip = tipOf(hops);
    // Sub-tasks name their parent by the task id, which is the root hop's id
    // — not necessarily the line key used above when the payload is old.
    const taskId = tip.taskId ?? null;
    const lines = taskId ? subs.get(`${tip.deviceId}::${taskId}`) : undefined;
    const subtasks = foldSubtasks(key, lines ?? new Map<string, T[]>(), view, (h) =>
      opts.isPending(h.deviceId, h.id),
    );
    const allSubHops = subtasks.flatMap((st) => st.hops);
    const state = taskStateOf({
      main: hops.map(view),
      subtasks: allSubHops.map(view),
      outcome: hops.find((h) => h.taskOutcome)?.taskOutcome ?? null,
    });
    const path = repoRootPath(tip.workspacePath);
    const device = opts.multiDevice ? opts.deviceLabelOf?.(tip.deviceId) : null;
    rows.push({
      key,
      deviceId: tip.deviceId,
      tip,
      hops,
      subtasks,
      state,
      title: taskTitle(tip, opts.untitled),
      projectKey: projectScopeKey(tip.deviceId, path, opts.multiDevice),
      projectName: device ? `${device} · ${tip.workspaceName}` : tip.workspaceName,
      mainPending: hops.some((h) => opts.isPending(h.deviceId, h.id)),
      lastActivityMs: Math.max(
        ...hops.map((h) => h.lastActivityMs),
        ...allSubHops.map((h) => h.lastActivityMs),
      ),
    });
  }
  rows.sort((a, b) => b.lastActivityMs - a.lastActivityMs);
  return rows;
}

/** Turn sub-task conversation lines into rows, newest activity first. */
function foldSubtasks<T extends SessionInfo>(
  keyPrefix: string,
  lines: Map<string, T[]>,
  view: (s: T) => TaskMemberView,
  isPending: (s: T) => boolean,
): Array<SubtaskRow<T>> {
  const out: Array<SubtaskRow<T>> = [];
  for (const [sk, hops] of lines) {
    out.push({
      key: `${keyPrefix}::${sk}`,
      tip: tipOf(hops),
      hops,
      state: taskStateOf({
        main: hops.map(view),
        subtasks: [],
        outcome: hops.find((h) => h.taskOutcome)?.taskOutcome ?? null,
      }),
      pending: hops.some(isPending),
    });
  }
  out.sort((a, b) => b.tip.lastActivityMs - a.tip.lastActivityMs);
  return out;
}

/**
 * The sub-tasks of one task, from a single device's snapshot — what the
 * detail page's 子任务 segment lists. Same folding as the task list's rows.
 */
export function subtaskRowsOf<T extends SessionInfo>(
  sessions: T[],
  taskId: string,
  opts: { isPending: (s: T) => boolean; isQuiet: (s: T) => boolean },
): Array<SubtaskRow<T>> {
  const lines = new Map<string, T[]>();
  for (const s of sessions) {
    if (s.isSubagent || s.parentTaskId !== taskId) continue;
    const k = lineKey(s);
    const arr = lines.get(k);
    if (arr) arr.push(s);
    else lines.set(k, [s]);
  }
  const view = (s: T): TaskMemberView => ({
    status: s.status,
    procAlive: !!s.procAlive,
    quiet: opts.isQuiet(s),
    hasPendingDecision: opts.isPending(s),
  });
  return foldSubtasks(taskId, lines, view, opts.isPending);
}

/** Every session id that makes up a task: its main hops (same `taskId`) and
 *  the hops of the sub-tasks it spawned. Artifacts carry a session id, so
 *  this is how a task's deliverables are found. */
export function taskSessionIds(sessions: SessionInfo[], session: SessionInfo): Set<string> {
  const ids = new Set<string>([session.id]);
  const taskId = session.taskId;
  if (!taskId) return ids;
  for (const s of sessions) {
    if (s.taskId === taskId || s.parentTaskId === taskId) ids.add(s.id);
  }
  return ids;
}

export interface TaskSectionRows<T extends Row = Row> {
  state: TaskState;
  rows: Array<TaskRow<T>>;
}

/** Cut rows into the five sections, in `TASK_STATES` order, keeping the input
 *  order inside each. Empty sections are dropped. */
export function sectionRows<T extends Row>(rows: Array<TaskRow<T>>): Array<TaskSectionRows<T>> {
  const by = new Map<TaskState, Array<TaskRow<T>>>();
  for (const r of rows) {
    const arr = by.get(r.state);
    if (arr) arr.push(r);
    else by.set(r.state, [r]);
  }
  return TASK_STATES.filter((st) => by.get(st)?.length).map((state) => ({
    state,
    rows: by.get(state)!,
  }));
}

/** `**P2** — run the tests` → `run the tests`: a step title without its id. */
export function stepText(raw: string | null | undefined): string | null {
  if (!raw) return null;
  const text = raw
    .replace(/^\s*\*{0,2}P\d+\*{0,2}\s*[—–-]\s*/, "")
    .replace(/\*\*/g, "")
    .trim();
  return text || null;
}

/** Facts the second line of a row is written from; the view turns them into
 *  copy. Kept as data so the choice of *what* to say is testable without i18n. */
export type RowDetail =
  | { kind: "subtaskNeedsYou"; subtaskTitle: string }
  | { kind: "needsYou" }
  | { kind: "error"; status: string; tool?: string }
  | { kind: "outOfCredits" }
  | { kind: "step"; done: number | null; total: number | null; text: string | null }
  | { kind: "wait"; reason: string }
  | { kind: "waitSubtasks"; active: number; total: number }
  | { kind: "review" }
  | { kind: "none" };

const ERROR_STATUSES = new Set(["rateLimited", "serverErrored", "remoteDisconnected", "stuck"]);

export function rowDetail(row: TaskRow, untitled: string): RowDetail {
  const tip = row.tip;
  const sum = tip.taskSummary ?? row.hops.find((h) => h.taskSummary)?.taskSummary ?? null;
  if (row.state === "needsYou") {
    if (row.mainPending) return { kind: "needsYou" };
    const sub = row.subtasks.find((st) => st.pending);
    if (sub) return { kind: "subtaskNeedsYou", subtaskTitle: taskTitle(sub.tip, untitled) };
    const errored =
      row.hops.find((h) => ERROR_STATUSES.has(h.status)) ??
      row.subtasks.flatMap((st) => st.hops).find((h) => ERROR_STATUSES.has(h.status));
    if (errored) {
      return { kind: "error", status: errored.status, tool: errored.stuckTool?.name };
    }
    return { kind: "needsYou" };
  }
  if (tip.outOfCredits && row.state !== "done") return { kind: "outOfCredits" };
  if (row.state === "running") {
    const plan = tip.taskPlan;
    return {
      kind: "step",
      done: plan ? plan.done : null,
      total: plan ? plan.total : null,
      text: stepText(sum?.currentStep ?? plan?.currentTask),
    };
  }
  if (row.state === "waiting") {
    if (sum?.waitReason) return { kind: "wait", reason: sum.waitReason };
    const watchNote = row.hops.flatMap((h) => h.watches ?? []).find((w) => w.note)?.note;
    if (watchNote) return { kind: "wait", reason: watchNote };
    const active = row.subtasks.filter(
      (st) => st.state === "running" || st.state === "waiting",
    ).length;
    if (row.subtasks.length > 0) {
      return { kind: "waitSubtasks", active, total: row.subtasks.length };
    }
    return { kind: "none" };
  }
  if (row.state === "review") return { kind: "review" };
  return { kind: "none" };
}

/** `子任务 2/3`: finished sub-tasks out of all. Null without sub-tasks. */
export function subtaskTally(row: TaskRow): { done: number; total: number } | null {
  if (row.subtasks.length === 0) return null;
  const done = row.subtasks.filter((st) => st.state === "review" || st.state === "done").length;
  return { done, total: row.subtasks.length };
}
