/** The task page's "recover interrupted sub-tasks" action, shared by the desktop
 *  rail and the phone's sub-task pane.
 *
 *  The wire shapes mirror `claw_fleet_core::subtask_recovery` (`SubtaskStatus`,
 *  `ResumeOutcome`); the classification itself is the core's, this only decides
 *  what the confirm list shows and what it pre-selects. */

export type RecoveryClass =
  | "recoverable"
  | "done"
  | "running"
  | "blocked"
  | "parked"
  | "userInterrupted"
  | "authFailed"
  | "failed"
  | "closed"
  | "unknown";

export interface SubtaskStatus {
  sessionId: string;
  /** The live end of the sub-task's own relay chain — what a resume targets. */
  effectiveId: string;
  spawnedBy: string;
  step?: string;
  workspace?: string;
  source?: string;
  phase: string;
  class: RecoveryClass;
  /** Why this class, one line, product copy (Chinese). */
  detail: string;
}

export interface ResumeOutcome {
  sessionId: string;
  ok: boolean;
  message: string;
}

/** Classes listed in the confirm list but not selectable: failures the boss
 *  should see, where a blind resume would fail again or undo a deliberate
 *  stop. Everything else that is not recoverable (done, closed, running,
 *  parked, still being retried, unreadable) stays out of the list. */
const SHOWN_DISABLED: ReadonlySet<RecoveryClass> = new Set<RecoveryClass>([
  "authFailed",
  "userInterrupted",
  "failed",
]);

export function recoverableCount(list: readonly SubtaskStatus[] | null | undefined): number {
  return (list ?? []).filter((s) => s.class === "recoverable").length;
}

export interface RecoveryRow {
  status: SubtaskStatus;
  /** Can be ticked; false rows are shown greyed with their reason. */
  selectable: boolean;
}

/** The confirm list: recoverable sub-tasks first, then the greyed failures,
 *  each group in the core's order. */
export function recoveryRows(list: readonly SubtaskStatus[]): RecoveryRow[] {
  const live = list.filter((s) => s.class === "recoverable");
  const greyed = list.filter((s) => SHOWN_DISABLED.has(s.class));
  return [
    ...live.map((status) => ({ status, selectable: true })),
    ...greyed.map((status) => ({ status, selectable: false })),
  ];
}

/** What the confirm list starts with ticked: every recoverable sub-task. */
export function defaultRecoverySelection(list: readonly SubtaskStatus[]): Set<string> {
  return new Set(list.filter((s) => s.class === "recoverable").map((s) => s.sessionId));
}

/** The ids to send, in list order, keeping only selectable rows that are ticked. */
export function selectedRecoveryIds(rows: readonly RecoveryRow[], picked: ReadonlySet<string>): string[] {
  return rows.filter((r) => r.selectable && picked.has(r.status.sessionId)).map((r) => r.status.sessionId);
}
