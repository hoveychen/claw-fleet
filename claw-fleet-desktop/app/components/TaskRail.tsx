// The right column of a session: what it is waiting on (its fleet watches and
// who waits on it), its task's sub-tasks and steps, the subagents
// running and the docs and side questions opened in it, the material the task
// produced. The session internals (model, tokens, relay chain, facets) are not
// here: they sit behind the header's ··· menu, where the developer view toggles
// them into the meta row. The header switch shows and hides this column.

import { useEffect, useMemo, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { BookOpen, CornerUpLeft, FileText, Image as ImageIcon, File } from "lucide-react";
import { useUIStore } from "../store";
import type { ReviewDoc, TaskItem } from "../types";
import { auxDocLabel, docId } from "../detailAux";
import { useIngestOpen } from "./blocks/ingestOpenContext";
import type { Artifact } from "./ArtifactsView";
import { rowMemberIds, sessionTitle, stepText, type SubtaskRow, type TaskRow } from "./homeRows";
import { STATE_DOT } from "./TaskList";
import { subtaskOutcome, type SubtaskOutcome } from "../../../shared-ts/subtaskOutcome";
import { SubtaskRecoveryBlock } from "./SubtaskRecovery";
import styles from "./TaskRail.module.css";

/** Dot colour of a stopped sub-task: only a clean report back is green. */
const OUTCOME_DOT: Record<SubtaskOutcome, string> = {
  reported: STATE_DOT.review,
  midTurn: "var(--color-warning)",
  interrupted: "var(--color-text-dim)",
  authFailed: "var(--color-error)",
  failed: "var(--color-error)",
};

const OUTCOME_LABEL: Record<SubtaskOutcome, [string, string]> = {
  reported: ["home.subtask_reported", "Reported back"],
  midTurn: ["home.subtask_mid_turn", "Stopped mid-turn"],
  interrupted: ["home.subtask_interrupted", "Interrupted by hand"],
  authFailed: ["home.subtask_auth_failed", "Auth failed"],
  failed: ["home.subtask_failed", "Permanent error"],
};

function ArtifactGlyph({ mime }: { mime: string }) {
  if (mime.startsWith("image/")) return <ImageIcon size={14} strokeWidth={1.8} />;
  if (mime.startsWith("text/") || mime.includes("markdown")) return <FileText size={14} strokeWidth={1.8} />;
  return <File size={14} strokeWidth={1.8} />;
}

/** What the task's sessions filed in the artifact library, newest first. Null
 *  until the first read lands, and while there is no task. Lives outside the
 *  rail because SessionDetail decides from it whether the rail has anything
 *  to show at all. */
export function useTaskArtifacts(row: TaskRow | null): Artifact[] | null {
  const [artifacts, setArtifacts] = useState<Artifact[] | null>(null);
  const memberKey = useMemo(() => (row ? [...rowMemberIds(row)].sort().join(",") : ""), [row]);
  useEffect(() => {
    setArtifacts(null);
    if (!memberKey) return;
    let alive = true;
    const members = new Set(memberKey.split(","));
    const load = () =>
      invoke<Artifact[]>("list_artifacts")
        .then((list) => {
          if (!alive) return;
          setArtifacts(
            (list ?? [])
              .filter((a) => a.sessionId != null && members.has(a.sessionId))
              .sort((a, b) => b.createdMs - a.createdMs),
          );
        })
        .catch(() => alive && setArtifacts([]));
    void load();
    // New deliverables arrive while the task runs.
    const timer = window.setInterval(load, 15_000);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [memberKey]);
  return artifacts;
}

/** Whether the task half of the rail has anything in it: sub-tasks (or a way
 *  back to the main conversation), plan steps, or filed material. Review docs
 *  attached to the session's decision cards count even without a task. */
export function taskRailHasContent(
  row: TaskRow | null,
  openSessionId: string,
  steps: TaskItem[] | null,
  artifacts: Artifact[] | null,
  reviewDocs: readonly ReviewDoc[] = [],
): boolean {
  if (reviewDocs.length > 0) return true;
  if (!row) return false;
  const inSubtask = !row.hops.some((h) => h.id === openSessionId);
  return (
    row.subtasks.length > 0 ||
    inSubtask ||
    (steps?.length ?? 0) > 0 ||
    (artifacts?.length ?? 0) > 0
  );
}

/** The step id (`P3`) a plan item's text starts with, if any. */
export function stepIdOf(text: string): string | null {
  return /^\s*\*{0,2}(P\d+[a-z]?)\*{0,2}/.exec(text)?.[1] ?? null;
}

/** Sub-tasks grouped under the step they were dispatched for (`fleet__spawn`
 *  `step`), keyed by step id. Only links into `planId` — the plan whose steps
 *  are on screen — count; the rest, and sub-tasks spawned without a step,
 *  come back in `loose`. */
export function groupSubtasksByStep(
  row: TaskRow,
  planId: string | null,
): { byStep: Map<string, SubtaskRow[]>; loose: SubtaskRow[] } {
  const links = row.hops.find((h) => h.taskSummary)?.taskSummary?.subtaskSteps ?? [];
  const stepOf = new Map(
    links.filter((l) => planId != null && l.planId === planId).map((l) => [l.taskId, l.step]),
  );
  const byStep = new Map<string, SubtaskRow[]>();
  const loose: SubtaskRow[] = [];
  for (const st of row.subtasks) {
    const step = st.tip.taskId ? stepOf.get(st.tip.taskId) : undefined;
    if (!step) {
      loose.push(st);
      continue;
    }
    const list = byStep.get(step);
    if (list) list.push(st);
    else byStep.set(step, [st]);
  }
  return { byStep, loose };
}

function reviewDocLabel(d: ReviewDoc): string {
  return d.title?.trim() || auxDocLabel(d.kind, d.ref);
}

/** The review docs across a session's decision cards — pending ones first,
 *  then answered ones from the decision history — one entry per doc. */
export function collectReviewDocs(
  pending: readonly { reviewDocs?: ReviewDoc[] | null }[],
  answered: readonly { reviewDocs?: ReviewDoc[] | null }[],
): ReviewDoc[] {
  const seen = new Set<string>();
  const out: ReviewDoc[] = [];
  for (const src of [pending, [...answered].reverse()]) {
    for (const card of src) {
      for (const d of card.reviewDocs ?? []) {
        const id = docId(d.kind, d.ref);
        if (seen.has(id)) continue;
        seen.add(id);
        out.push(d);
      }
    }
  }
  return out;
}

export function TaskRail({
  row,
  openSessionId,
  steps,
  stepsPlanId = null,
  artifacts,
  reviewDocs = [],
  onOpenSession,
  waiting,
  waitingCount = 0,
  ambient,
  empty,
  className,
}: {
  /** The task the open session belongs to; null on a session with no task,
   *  where only the ambient sections apply. */
  row: TaskRow | null;
  /** The session the conversation column shows: the main line, or a sub-task. */
  openSessionId: string;
  /** The task's plan items, or null when it has no plan. */
  steps: TaskItem[] | null;
  /** Id of the plan `steps` come from; sub-tasks dispatched for one of its
   *  steps are listed under that step. */
  stepsPlanId?: string | null;
  /** See useTaskArtifacts. */
  artifacts: Artifact[] | null;
  /** Docs the session's decision cards asked the reader to review (`reviewDocs`
   *  on fleet__ask), deduped. Listed with the material instead of inside the
   *  card, so a card stays a question and the doc opens in the reader. */
  reviewDocs?: readonly ReviewDoc[];
  onOpenSession: (sessionId: string) => void;
  /** The open session's watches and waiters, already rendered; the rail only
   *  frames them. Omitted when there are none. */
  waiting?: ReactNode;
  /** How many watches and waiters `waiting` lists, for the heading. */
  waitingCount?: number;
  /** Running subagents, opened docs and side questions — SessionAuxRail. */
  ambient: ReactNode;
  /** Nothing in any section: the header switch is holding the rail open. */
  empty: boolean;
  /** Host-specific placement, e.g. the standalone pane's caption inset. */
  className?: string;
}) {
  const { t } = useTranslation();
  const untitled = t("history.untitled");
  const requestArtifactNav = useUIStore((s) => s.requestArtifactNav);
  /* A material row opens like a transcript's ingest card: the first click
     expands it into the floating reader beside the conversation, and only a
     click on the one already open there goes to the artifacts page. Outside a
     provider (tests) it goes straight to the page. */
  const ingest = useIngestOpen();
  const openMaterial = (a: Artifact) => {
    const title = a.title || a.name;
    if (ingest && ingest.expandedId !== docId("artifact", a.id)) ingest.open("artifact", a.id, title);
    else requestArtifactNav(a.id);
  };
  const openReviewDoc = (d: ReviewDoc) => {
    if (!ingest) return;
    const id = docId(d.kind, d.ref);
    if (ingest.expandedId === id) return;
    ingest.open(d.kind, d.ref, reviewDocLabel(d));
  };
  const materialCount = (artifacts?.length ?? 0) + reviewDocs.length;

  const inSubtask = row != null && !row.hops.some((h) => h.id === openSessionId);
  const subtasks = row?.subtasks ?? [];
  const doneSubtasks = subtasks.filter(
    (st) => st.state === "review" || st.state === "done",
  ).length;
  const doneSteps = steps?.filter((s) => s.done).length ?? 0;
  const current = steps?.findIndex((s) => !s.done) ?? -1;
  const showSteps = !!steps && steps.length > 0;
  const { byStep, loose } = row && showSteps
    ? groupSubtasksByStep(row, stepsPlanId)
    : { byStep: new Map<string, SubtaskRow[]>(), loose: subtasks };
  // Once any sub-task hangs under a step, the steps lead and the sub-task
  // block below keeps only the ones that hang under none.
  const linked = byStep.size > 0;
  const listedSubtasks = linked ? loose : subtasks;

  const subtaskButton = (st: SubtaskRow) => {
    const plan = st.tip.taskPlan;
    const open = st.hops.some((h) => h.id === openSessionId);
    const outcome = subtaskOutcome(st.state, st.tip.lastTurnEnd);
    return (
      <button
        key={st.key}
        type="button"
        className={`${styles.item} ${open ? styles.item_open : ""}`}
        onClick={() => onOpenSession(st.tip.id)}
        aria-current={open || undefined}
      >
        <span
          className={styles.dot}
          style={{ background: outcome ? OUTCOME_DOT[outcome] : STATE_DOT[st.state] }}
        />
        <span className={styles.item_title}>{sessionTitle(st.tip, untitled)}</span>
        <span className={styles.item_meta}>
          {/* A stopped sub-task reports to the main conversation, not
              to the boss, so "review" is not something to accept — and
              a stop that was not a clean end says how it stopped. */}
          {outcome
            ? t(OUTCOME_LABEL[outcome][0], OUTCOME_LABEL[outcome][1])
            : t(`home.section_${st.state}`)}
        </span>
        {plan && plan.total > 0 && (
          <span className={styles.item_meta}>
            {plan.done}/{plan.total}
          </span>
        )}
      </button>
    );
  };

  const stepsSection = showSteps && (
    <section className={styles.block}>
      <h3 className={styles.head}>
        {t("home.rail_steps", "Steps")}
        <span className={styles.count}>
          {doneSteps}/{steps.length}
        </span>
      </h3>
      <ol className={styles.steps}>
        {steps.map((s, i) => {
          const id = stepIdOf(s.text);
          const onIt = (id && byStep.get(id)) || [];
          return (
            <li
              key={i}
              className={`${styles.step} ${s.done ? styles.step_done : ""} ${i === current ? styles.step_current : ""}`}
            >
              <span className={styles.step_mark} aria-hidden>
                {s.done && (
                  <svg viewBox="0 0 14 14" className={styles.step_check}>
                    <path d="M4.2 7.2l1.9 1.9 3.8-4" />
                  </svg>
                )}
              </span>
              <span className={styles.step_text}>
                {stepText(s.text) ?? s.text}
                {i === current && (
                  <span className={styles.step_now}>{t("home.rail_step_now", "(now)")}</span>
                )}
              </span>
              {onIt.length > 0 && (
                <div className={`${styles.card} ${styles.step_subtasks}`} data-testid="step-subtasks">
                  {onIt.map(subtaskButton)}
                </div>
              )}
            </li>
          );
        })}
      </ol>
    </section>
  );

  return (
    <aside className={`${styles.rail} ${className ?? ""}`} data-testid="task-rail">
      {/* Held open by the switch with nothing in it. Saying so beats a blank
          column, which reads as the rail having failed to load. */}
      {empty && <p className={styles.empty}>{t("detail.rail_empty", "暂无运行中的 Agent 或已打开的文档")}</p>}

      {/* First: while it is set, the wait is what the session is doing. */}
      {waiting && (
        <section className={styles.block} data-testid="rail-waiting">
          <h3 className={styles.head}>
            {t("home.rail_waiting", "Waiting on")}
            <span className={styles.count}>{waitingCount}</span>
          </h3>
          {waiting}
        </section>
      )}

      {/* Above the steps and sub-tasks: while a sub-task is dead mid-turn,
          recovering it is the one thing to do on this task. */}
      {row && subtasks.length > 0 && (
        <SubtaskRecoveryBlock
          coordinatorId={row.tip.id}
          titleOf={(id) => {
            const st = subtasks.find((x) => x.hops.some((h) => h.id === id));
            return st ? sessionTitle(st.tip, untitled) : null;
          }}
        />
      )}

      {linked && stepsSection}

      {row && (listedSubtasks.length > 0 || inSubtask) && (
        <section className={styles.block}>
          <h3 className={styles.head}>
            {linked ? t("home.rail_subtasks_other", "Other sub-tasks") : t("home.rail_subtasks", "Sub-tasks")}
            <span className={styles.count}>
              {linked ? listedSubtasks.length : `${doneSubtasks}/${subtasks.length}`}
            </span>
          </h3>
          <div className={styles.card}>
            {inSubtask && (
              <button
                type="button"
                className={styles.item}
                onClick={() => onOpenSession(row.tip.id)}
              >
                <CornerUpLeft size={13} strokeWidth={1.8} className={styles.item_icon} />
                <span className={styles.item_title}>{t("home.rail_main", "Main conversation")}</span>
              </button>
            )}
            {listedSubtasks.map(subtaskButton)}
          </div>
        </section>
      )}

      {!linked && stepsSection}

      {ambient}

      {materialCount > 0 && (
        <section className={styles.block}>
          <h3 className={styles.head}>
            {t("home.rail_material", "Material")}
            <span className={styles.count}>{materialCount}</span>
          </h3>
          <div className={styles.card}>
            {reviewDocs.map((d) => {
              const id = docId(d.kind, d.ref);
              const open = ingest?.expandedId === id;
              return (
                <button
                  key={id}
                  type="button"
                  className={`${styles.item} ${open ? styles.item_open : ""}`}
                  onClick={() => openReviewDoc(d)}
                  aria-current={open || undefined}
                  title={d.ref}
                >
                  <span className={styles.item_icon}>
                    {d.kind === "wiki" ? (
                      <BookOpen size={14} strokeWidth={1.8} />
                    ) : (
                      <FileText size={14} strokeWidth={1.8} />
                    )}
                  </span>
                  <span className={styles.item_title}>{reviewDocLabel(d)}</span>
                </button>
              );
            })}
            {(artifacts ?? []).map((a) => {
              const open = ingest?.expandedId === docId("artifact", a.id);
              return (
              <button
                key={a.id}
                type="button"
                className={`${styles.item} ${styles.item_multiline} ${open ? styles.item_open : ""}`}
                onClick={() => openMaterial(a)}
                aria-current={open || undefined}
                title={a.note ? `${a.title || a.name}\n${a.note}` : a.title || a.name}
              >
                <span className={styles.item_icon}>
                  <ArtifactGlyph mime={a.mime} />
                </span>
                <span className={`${styles.item_title} ${styles.item_title_wrap}`}>{a.title || a.name}</span>
              </button>
              );
            })}
          </div>
        </section>
      )}
    </aside>
  );
}
