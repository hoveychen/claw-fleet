// Folding the session scan into the desktop home's task list.
//
// A task is a main conversation (every relay hop of it) plus the sub-tasks it
// spawned. The scan is flat — one entry per session — so the list regroups it:
// main hops by `taskId`, sub-task hops under their `parentTaskId`. The section a
// task is filed under comes from `shared-ts/taskState.ts`, the derivation the
// phone's task list uses too, so both clients put a task under the same heading.
//
// A pending decision card whose session belongs to no listed task (a `claude`
// opened by hand that tripped the guard, a session the scan cannot see) gets a
// row of its own in "needs you": the home is where every card is answered, and
// a card with no row would have no way in.

import { repoRootPath } from "../../../shared-ts/repoPath";
import {
  subtaskStateOf,
  taskStateOf,
  type TaskMemberView,
  type TaskState,
  type TaskView,
} from "../../../shared-ts/taskState";
import { isFleetOwnedTask, type PendingDecision, type SessionInfo } from "../types";

/**
 * Section order on the desktop home: what needs you, what is moving, what is
 * parked, what is ready to accept, what is closed.
 *
 * Deliberately not `TASK_STATES` (which puts review second): the signed-off
 * home design reads top-down from "act now" to "look at later", and accepting a
 * result is a sweep the boss does in a batch, so it sits below the live work.
 */
export const HOME_SECTION_ORDER: TaskState[] = [
  "needsYou",
  "running",
  "waiting",
  "review",
  "done",
];

export interface SubtaskRow {
  key: string;
  /** The hop to open: the sub-task's latest. */
  tip: SessionInfo;
  hops: SessionInfo[];
  state: TaskState;
  /** A card from this sub-task is waiting for an answer. */
  pending: boolean;
}

export interface TaskRow {
  kind: "task";
  key: string;
  /** The main-conversation hop to open: the latest one. */
  tip: SessionInfo;
  hops: SessionInfo[];
  subtasks: SubtaskRow[];
  state: TaskState;
  title: string;
  /** Repository root, the project scope key. */
  projectPath: string;
  projectName: string;
  /** A card from the main conversation is waiting. */
  mainPending: boolean;
  startedAtMs: number;
  lastActivityMs: number;
}

/** A card with no task row to hang under. Always in "needs you". */
export interface CardRow {
  kind: "card";
  key: string;
  sessionId: string;
  /** The scanned session, when the scan has it (a hand-opened `claude`). */
  session: SessionInfo | null;
  decisions: PendingDecision[];
  state: "needsYou";
  title: string;
  projectPath: string | null;
  projectName: string;
  startedAtMs: number;
  lastActivityMs: number;
}

export type HomeRow = TaskRow | CardRow;

/** Hop order inside one conversation line: relay hop number, then recency. */
function laterHop(a: SessionInfo, b: SessionInfo): boolean {
  const ha = a.handoff?.hop ?? 0;
  const hb = b.handoff?.hop ?? 0;
  if (ha !== hb) return ha > hb;
  return a.lastActivityMs > b.lastActivityMs;
}

function tipOf(hops: SessionInfo[]): SessionInfo {
  return hops.reduce((a, b) => (laterHop(b, a) ? b : a));
}

/** Which conversation line a session belongs to. `taskId` is the core's
 *  answer; a session scanned before the task model existed lacks it, and a
 *  relay chain is then the best stand-in for "one line". */
function lineKey(s: SessionInfo): string {
  if (s.taskId) return s.taskId;
  if (s.handoff && s.handoff.chainLen > 1) return `chain:${s.handoff.chainId}`;
  return s.id;
}

export function sessionTitle(s: SessionInfo, untitled: string): string {
  return s.titleOverride || s.aiTitle || s.slug || untitled;
}

export interface BuildOptions {
  /** The caller's latched quiet-alive verdict (`isQuietAliveSticky`). Passed
   *  in because the latch is stateful and belongs to the view. */
  isQuiet: (s: SessionInfo) => boolean;
  /** The caller's latched "just woken" verdict (`isWakingSticky`), given
   *  whether a card from the session is pending. Stateful like `isQuiet`;
   *  absent means never waking. */
  isWaking?: (s: SessionInfo, pending: boolean) => boolean;
  untitled: string;
}

/**
 * Regroup the scan into home rows, newest activity first. Only Fleet-started
 * main conversations become task rows; a sub-task hop joins the task named by
 * its `parentTaskId`. Cards from sessions outside every task become card rows.
 */
export function buildHomeRows(
  sessions: SessionInfo[],
  decisions: PendingDecision[],
  opts: BuildOptions,
): HomeRow[] {
  const pendingIds = new Set(decisions.map((d) => d.request.sessionId));
  const isPending = (s: SessionInfo) => pendingIds.has(s.id);

  const mains = new Map<string, SessionInfo[]>();
  const subs = new Map<string, Map<string, SessionInfo[]>>();
  for (const s of sessions) {
    if (s.isSubagent) continue;
    if (s.parentTaskId) {
      let lines = subs.get(s.parentTaskId);
      if (!lines) subs.set(s.parentTaskId, (lines = new Map()));
      push(lines, lineKey(s), s);
      continue;
    }
    if (!isFleetOwnedTask(s)) continue;
    push(mains, lineKey(s), s);
  }

  const view = (s: SessionInfo, superseded: boolean): TaskMemberView => ({
    status: s.status,
    procAlive: !!s.procAlive,
    quiet: opts.isQuiet(s),
    hasPendingDecision: isPending(s),
    superseded,
    waking: opts.isWaking?.(s, isPending(s)) ?? false,
  });

  const covered = new Set<string>();
  const rows: HomeRow[] = [];
  for (const [key, hops] of mains) {
    const tip = tipOf(hops);
    // Sub-tasks name their parent by the task id, which is the root hop's id —
    // not necessarily the line key when the session predates the task model.
    const taskId = tip.taskId ?? null;
    const lines = taskId ? subs.get(taskId) : undefined;
    const outcome = hops.find((h) => h.taskOutcome)?.taskOutcome ?? null;
    const subtasks = foldSubtasks(key, lines ?? new Map(), view, isPending, outcome);
    const allSubHops = subtasks.flatMap((st) => st.hops);
    for (const h of hops) covered.add(h.id);
    for (const h of allSubHops) covered.add(h.id);
    const summary = tip.taskSummary ?? hops.find((h) => h.taskSummary)?.taskSummary;
    rows.push({
      kind: "task",
      key,
      tip,
      hops,
      subtasks,
      state: taskStateOf({
        main: hops.map((h) => view(h, h !== tip)),
        subtasks: subtasks.flatMap((st) => st.hops.map((h) => view(h, h !== st.tip))),
        outcome,
      }),
      title: sessionTitle(tip, opts.untitled),
      projectPath: repoRootPath(tip.workspacePath),
      projectName: tip.workspaceName,
      mainPending: hops.some(isPending),
      startedAtMs: summary?.startedAtMs || Math.min(...hops.map((h) => h.createdAtMs)),
      lastActivityMs: Math.max(
        ...hops.map((h) => h.agentLastActivityMs || h.lastActivityMs),
        ...allSubHops.map((h) => h.lastActivityMs),
      ),
    });
  }

  const byId = new Map(sessions.map((s) => [s.id, s]));
  const cardsBySession = new Map<string, PendingDecision[]>();
  for (const d of decisions) {
    if (covered.has(d.request.sessionId)) continue;
    const arr = cardsBySession.get(d.request.sessionId);
    if (arr) arr.push(d);
    else cardsBySession.set(d.request.sessionId, [d]);
  }
  for (const [sessionId, cards] of cardsBySession) {
    const session = byId.get(sessionId) ?? null;
    const first = cards[0].request;
    const arrived = Math.min(...cards.map((d) => d.arrivedAt));
    rows.push({
      kind: "card",
      key: `card:${sessionId}`,
      sessionId,
      session,
      decisions: cards,
      state: "needsYou",
      title: session
        ? sessionTitle(session, first.aiTitle || opts.untitled)
        : first.aiTitle || opts.untitled,
      projectPath: session ? repoRootPath(session.workspacePath) : null,
      projectName: session?.workspaceName ?? first.workspaceName,
      startedAtMs: session?.createdAtMs ?? arrived,
      lastActivityMs: Math.max(arrived, session?.lastActivityMs ?? 0),
    });
  }

  rows.sort((a, b) => b.lastActivityMs - a.lastActivityMs);
  return rows;
}

function push<K, V>(m: Map<K, V[]>, k: K, v: V) {
  const arr = m.get(k);
  if (arr) arr.push(v);
  else m.set(k, [v]);
}

/** Turn sub-task conversation lines into rows, newest activity first. */
function foldSubtasks(
  keyPrefix: string,
  lines: Map<string, SessionInfo[]>,
  view: (s: SessionInfo, superseded: boolean) => TaskMemberView,
  isPending: (s: SessionInfo) => boolean,
  parentOutcome: TaskView["outcome"],
): SubtaskRow[] {
  const out: SubtaskRow[] = [];
  for (const [sk, hops] of lines) {
    const tip = tipOf(hops);
    out.push({
      key: `${keyPrefix}::${sk}`,
      tip,
      hops,
      state: subtaskStateOf(
        {
          main: hops.map((h) => view(h, h !== tip)),
          subtasks: [],
          outcome: hops.find((h) => h.taskOutcome)?.taskOutcome ?? null,
        },
        parentOutcome,
      ),
      pending: hops.some(isPending),
    });
  }
  out.sort((a, b) => b.tip.lastActivityMs - a.tip.lastActivityMs);
  return out;
}

/** The task row a session belongs to — as a main hop or as a sub-task hop. */
export function rowOfSession(rows: HomeRow[], sessionId: string): HomeRow | null {
  for (const r of rows) {
    if (r.kind === "card") {
      if (r.sessionId === sessionId) return r;
      continue;
    }
    if (r.hops.some((h) => h.id === sessionId)) return r;
    if (r.subtasks.some((st) => st.hops.some((h) => h.id === sessionId))) return r;
  }
  return null;
}

export interface RowFilter {
  /** Repository root, or null for every project. */
  scopedPath: string | null;
  query: string;
  /** Transcript paths the full-text index matched for `query`. */
  ftsMatchPaths: Set<string>;
}

function sessionMatches(s: SessionInfo, q: string, fts: Set<string>): boolean {
  return (
    (s.titleOverride?.toLowerCase().includes(q) ?? false) ||
    (s.aiTitle?.toLowerCase().includes(q) ?? false) ||
    (s.slug?.toLowerCase().includes(q) ?? false) ||
    (s.lastMessagePreview?.toLowerCase().includes(q) ?? false) ||
    s.workspaceName.toLowerCase().includes(q) ||
    // The attributed TASKS.md plan — id, title and current step — so a task is
    // reachable by the name its plan goes by.
    (s.taskPlan?.planId?.toLowerCase().includes(q) ?? false) ||
    (s.taskPlan?.currentPlan?.toLowerCase().includes(q) ?? false) ||
    (s.taskPlan?.currentTask?.toLowerCase().includes(q) ?? false) ||
    fts.has(s.jsonlPath)
  );
}

/** Project scope and search. A task matches when any of its sessions (main
 *  hops or sub-tasks) does: searching a sub-task's words finds its task. */
export function filterRows(rows: HomeRow[], f: RowFilter): HomeRow[] {
  const q = f.query.trim().toLowerCase();
  return rows.filter((r) => {
    if (f.scopedPath && r.projectPath !== f.scopedPath) return false;
    if (!q) return true;
    if (r.title.toLowerCase().includes(q) || r.projectName.toLowerCase().includes(q)) {
      return true;
    }
    const members =
      r.kind === "card"
        ? r.session
          ? [r.session]
          : []
        : [...r.hops, ...r.subtasks.flatMap((st) => st.hops)];
    return members.some((s) => sessionMatches(s, q, f.ftsMatchPaths));
  });
}

export interface HomeSection {
  state: TaskState;
  rows: HomeRow[];
}

/** Cut rows into sections in `HOME_SECTION_ORDER`, keeping input order inside
 *  each. Empty sections are dropped. */
export function sectionRows(rows: HomeRow[]): HomeSection[] {
  const by = new Map<TaskState, HomeRow[]>();
  for (const r of rows) push(by, r.state, r);
  return HOME_SECTION_ORDER.filter((st) => by.get(st)?.length).map((state) => ({
    state,
    rows: by.get(state)!,
  }));
}

/** `**P2** — run the tests` → `run the tests`: a step title without its id. */
export function stepText(raw: string | null | undefined): string | null {
  if (!raw) return null;
  const text = raw
    .replace(/^\s*\*{0,2}P\d+[a-z]?\*{0,2}\s*[—–-]\s*/, "")
    .replace(/\*\*/g, "")
    .replace(/`/g, "")
    .trim();
  return text || null;
}

/** Facts the second line of a row is written from; the view turns them into
 *  copy. Data, so *what* a row says is testable without i18n. */
export type RowDetail =
  | { kind: "subtaskNeedsYou"; subtaskTitle: string }
  | { kind: "needsYou" }
  | { kind: "error"; status: string; tool?: string }
  | { kind: "outOfCredits" }
  | {
      kind: "step";
      done: number | null;
      total: number | null;
      text: string | null;
      subtasks: { done: number; total: number } | null;
    }
  | { kind: "wait"; reason: string; sinceMs: number }
  | { kind: "waitSubtasks"; active: number; total: number; sinceMs: number }
  | { kind: "review"; done: number | null; total: number | null }
  | { kind: "none" };

const ERROR_STATUSES = new Set(["rateLimited", "serverErrored", "remoteDisconnected", "stuck"]);

export function rowDetail(row: HomeRow, untitled: string): RowDetail {
  if (row.kind === "card") return { kind: "needsYou" };
  const tip = row.tip;
  const sum = tip.taskSummary ?? row.hops.find((h) => h.taskSummary)?.taskSummary ?? null;
  const plan = tip.taskPlan ?? row.hops.find((h) => h.taskPlan)?.taskPlan ?? null;
  if (row.state === "needsYou") {
    if (row.mainPending) return { kind: "needsYou" };
    const sub = row.subtasks.find((st) => st.pending);
    if (sub) return { kind: "subtaskNeedsYou", subtaskTitle: sessionTitle(sub.tip, untitled) };
    const errored =
      row.hops.find((h) => ERROR_STATUSES.has(h.status)) ??
      row.subtasks.flatMap((st) => st.hops).find((h) => ERROR_STATUSES.has(h.status));
    if (errored) return { kind: "error", status: errored.status, tool: errored.stuckTool?.name };
    return { kind: "needsYou" };
  }
  if (tip.outOfCredits && row.state !== "done") return { kind: "outOfCredits" };
  if (row.state === "running") {
    return {
      kind: "step",
      done: plan ? plan.done : null,
      total: plan ? plan.total : null,
      text: stepText(sum?.currentStep ?? plan?.currentTask),
      subtasks: subtaskTally(row),
    };
  }
  if (row.state === "waiting") {
    // Parked since the main line last did anything.
    const sinceMs = Math.max(...row.hops.map((h) => h.lastActivityMs));
    if (sum?.waitReason) return { kind: "wait", reason: sum.waitReason, sinceMs };
    const watchNote = row.hops.flatMap((h) => h.watches ?? []).find((w) => w.note)?.note;
    if (watchNote) return { kind: "wait", reason: watchNote, sinceMs };
    if (row.subtasks.length > 0) {
      const active = row.subtasks.filter(
        (st) => st.state === "running" || st.state === "waiting",
      ).length;
      return { kind: "waitSubtasks", active, total: row.subtasks.length, sinceMs };
    }
    return { kind: "none" };
  }
  if (row.state === "review") {
    return { kind: "review", done: plan ? plan.done : null, total: plan ? plan.total : null };
  }
  return { kind: "none" };
}

/** `子任务 2/3`: finished sub-tasks out of all. Null without sub-tasks. */
export function subtaskTally(row: TaskRow): { done: number; total: number } | null {
  if (row.subtasks.length === 0) return null;
  const done = row.subtasks.filter((st) => st.state === "review" || st.state === "done").length;
  return { done, total: row.subtasks.length };
}

/** The first row a fresh window should land on: the newest "needs you". */
export function firstNeedsYou(rows: HomeRow[]): HomeRow | null {
  return rows.find((r) => r.state === "needsYou") ?? null;
}

/** The session id a click on the row opens. */
export function rowSessionId(row: HomeRow): string {
  return row.kind === "card" ? row.sessionId : row.tip.id;
}

/** Every session id behind a row: main hops and every sub-task hop. */
export function rowMemberIds(row: HomeRow): Set<string> {
  if (row.kind === "card") return new Set([row.sessionId]);
  return new Set([
    ...row.hops.map((h) => h.id),
    ...row.subtasks.flatMap((st) => st.hops.map((h) => h.id)),
  ]);
}

/** The cards the task behind `row` is waiting on, oldest first, leaving out
 *  those of `exceptSessionId` (the open conversation reads its own). */
export function rowDecisions(
  row: HomeRow,
  decisions: PendingDecision[],
  exceptSessionId: string | null = null,
): PendingDecision[] {
  const members = rowMemberIds(row);
  return decisions.filter(
    (d) => members.has(d.request.sessionId) && d.request.sessionId !== exceptSessionId,
  );
}

/** Where to go once the open task has nothing left to answer: the first other
 *  row, in the order shown, that still needs you. */
export function nextNeedsYou(rows: HomeRow[], currentKey: string | null): HomeRow | null {
  return rows.find((r) => r.state === "needsYou" && r.key !== currentKey) ?? null;
}
