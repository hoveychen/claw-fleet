/**
 * The five states a task is shown in, shared by the desktop home and the phone
 * so both cut the task list into the same sections in the same order.
 *
 * A task is judged as a whole: its main conversation (every relay hop of it)
 * plus every sub-task it spawned. Per-session run status is an input, not the
 * answer — a task whose main hop has gone quiet while two sub-tasks work is
 * "waiting", not "ended".
 *
 * Order is the order the boss reads in: what needs them, what is ready to
 * accept, what is moving, what is parked, what is closed.
 */
export type TaskState = "needsYou" | "review" | "running" | "waiting" | "done";

export const TASK_STATES: TaskState[] = [
  "needsYou",
  "review",
  "running",
  "waiting",
  "done",
];

/** One session of a task, as the state derivation needs to see it. */
export interface TaskMemberView {
  /** `SessionInfo.status`. */
  status: string;
  procAlive: boolean;
  /** The caller's latched "process alive but transcript gone quiet" verdict.
   *  Passed in because the latch keeps state across scans, which a pure
   *  function cannot. */
  quiet: boolean;
  /** A decision card from this session is waiting for an answer. */
  hasPendingDecision: boolean;
  /** A later relay hop of the same conversation line took over from this one,
   *  so its run status is history: an error it ended on was already answered
   *  by the takeover. A card it left waiting still counts. */
  superseded?: boolean;
  /** The caller's latched "just woken" verdict (`shared-ts/wakeLatch.ts`):
   *  its card was just answered, its watch just fired or its process just
   *  started, and the transcript has not caught up yet. Passed in for the
   *  same reason as `quiet`. */
  waking?: boolean;
}

export interface TaskView {
  /** Hops of the main conversation. */
  main: TaskMemberView[];
  /** Sessions of every sub-task, all hops. */
  subtasks: TaskMemberView[];
  /** `SessionInfo.taskOutcome` of the task (any hop): the boss closed it. */
  outcome: "completed" | "abandoned" | null | undefined;
}

/** Statuses that mean the agent is wedged or cut off and needs a human. */
const ERROR_STATUSES = new Set([
  "rateLimited",
  "serverErrored",
  "remoteDisconnected",
  "stuck",
]);

/** Statuses that mean a turn is genuinely in flight. */
const WORKING_STATUSES = new Set([
  "thinking",
  "executing",
  "streaming",
  "processing",
  "active",
  "delegating",
]);

function needsHuman(m: TaskMemberView): boolean {
  return m.hasPendingDecision || (!m.superseded && ERROR_STATUSES.has(m.status));
}

/** A turn is in flight. A parked process with a pending card is not "working";
 *  that case is caught earlier as needsYou. A session that was just woken is,
 *  even while its status still reads the previous turn's `waitingInput`. */
function isWorking(m: TaskMemberView): boolean {
  return (
    WORKING_STATUSES.has(m.status) ||
    m.quiet ||
    !!m.waking ||
    (m.procAlive && m.status !== "waitingInput")
  );
}

function isWatching(m: TaskMemberView): boolean {
  return m.status === "watching";
}

/**
 * Which section a task belongs to.
 *
 * 1. needsYou — any session of the task (main or sub-task) has a card waiting
 *    or is in an error state (errors only on the latest hop of each line —
 *    see `superseded`). Checked first so a card is never hidden under a
 *    closed task. An error is not: once the boss closed the task (marked it
 *    done from the list, say, after it hit a rate limit), the error is
 *    answered.
 * 2. done — the boss pressed finish / abandon.
 * 3. running — the main conversation has a turn in flight.
 * 4. waiting — the main conversation is parked on a `fleet watch`, or its
 *    sub-tasks are still working / watching.
 * 5. review — nothing is moving and the task is not closed: the chain stopped
 *    and the result is waiting to be accepted.
 */
export function taskStateOf(task: TaskView): TaskState {
  const all = [...task.main, ...task.subtasks];
  const closed = task.outcome === "completed" || task.outcome === "abandoned";
  if (all.some((m) => (closed ? m.hasPendingDecision : needsHuman(m)))) return "needsYou";
  if (closed) return "done";
  if (task.main.some(isWorking)) return "running";
  if (task.main.some(isWatching) || task.subtasks.some((m) => isWorking(m) || isWatching(m))) {
    return "waiting";
  }
  return "review";
}

/**
 * The state of one sub-task line. Same rules as `taskStateOf`, plus one: a
 * sub-task that stopped and is waiting to be accepted follows its parent once
 * the boss closes the parent. Accepting or abandoning a task stamps only the
 * main conversation, so without this every finished sub-task under a closed
 * task stays in "review" forever. A sub-task that is still moving or holds a
 * card keeps its own state.
 */
export function subtaskStateOf(task: TaskView, parentOutcome: TaskView["outcome"]): TaskState {
  const state = taskStateOf(task);
  if (state === "review" && (parentOutcome === "completed" || parentOutcome === "abandoned")) {
    return "done";
  }
  return state;
}

/** Rank for sorting by state, in `TASK_STATES` order. */
export function taskStateRank(state: TaskState): number {
  return TASK_STATES.indexOf(state);
}
