// The right column of a session: its task's sub-tasks and steps, the subagents
// running and the docs and side questions opened in it, the material the task
// produced, and — folded away under "developer view" — the session internals
// (tokens, notes, temp files, background commands, workflow, relay chain).
// The one right-hand surface; the header switch shows and hides all of it.

import { useEffect, useMemo, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { ChevronRight, CornerUpLeft, FileText, Image as ImageIcon, File } from "lucide-react";
import { useUIStore } from "../store";
import type { TaskItem } from "../types";
import type { Artifact } from "./ArtifactsView";
import { rowMemberIds, sessionTitle, stepText, type TaskRow } from "./homeRows";
import { STATE_DOT } from "./TaskList";
import styles from "./TaskRail.module.css";

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
 *  back to the main conversation), plan steps, or filed material. */
export function taskRailHasContent(
  row: TaskRow | null,
  openSessionId: string,
  steps: TaskItem[] | null,
  artifacts: Artifact[] | null,
): boolean {
  if (!row) return false;
  const inSubtask = !row.hops.some((h) => h.id === openSessionId);
  return (
    row.subtasks.length > 0 ||
    inSubtask ||
    (steps?.length ?? 0) > 0 ||
    (artifacts?.length ?? 0) > 0
  );
}

export function TaskRail({
  row,
  openSessionId,
  steps,
  artifacts,
  onOpenSession,
  ambient,
  empty,
  developer,
  className,
}: {
  /** The task the open session belongs to; null on a session with no task,
   *  where only the ambient sections and the developer view apply. */
  row: TaskRow | null;
  /** The session the conversation column shows: the main line, or a sub-task. */
  openSessionId: string;
  /** The task's plan items, or null when it has no plan. */
  steps: TaskItem[] | null;
  /** See useTaskArtifacts. */
  artifacts: Artifact[] | null;
  onOpenSession: (sessionId: string) => void;
  /** Running subagents, opened docs and side questions — SessionAuxRail. */
  ambient: ReactNode;
  /** Nothing in any section: the header switch is holding the rail open. */
  empty: boolean;
  /** The developer view's content, built by the conversation pane that owns
   *  the facets it opens. */
  developer: ReactNode;
  /** Host-specific placement, e.g. the standalone pane's caption inset. */
  className?: string;
}) {
  const { t } = useTranslation();
  const untitled = t("history.untitled");
  const requestArtifactNav = useUIStore((s) => s.requestArtifactNav);
  const [devOpen, setDevOpen] = useState(false);

  const inSubtask = row != null && !row.hops.some((h) => h.id === openSessionId);
  const subtasks = row?.subtasks ?? [];
  const doneSubtasks = subtasks.filter(
    (st) => st.state === "review" || st.state === "done",
  ).length;
  const doneSteps = steps?.filter((s) => s.done).length ?? 0;
  const current = steps?.findIndex((s) => !s.done) ?? -1;

  return (
    <aside className={`${styles.rail} ${className ?? ""}`} data-testid="task-rail">
      {/* Held open by the switch with nothing in it. Saying so beats a blank
          column, which reads as the rail having failed to load. */}
      {empty && <p className={styles.empty}>{t("detail.rail_empty", "暂无运行中的 Agent 或已打开的文档")}</p>}

      {row && (subtasks.length > 0 || inSubtask) && (
        <section className={styles.block}>
          <h3 className={styles.head}>
            {t("home.rail_subtasks", "Sub-tasks")}
            <span className={styles.count}>
              {doneSubtasks}/{subtasks.length}
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
            {subtasks.map((st) => {
              const plan = st.tip.taskPlan;
              const open = st.hops.some((h) => h.id === openSessionId);
              return (
                <button
                  key={st.key}
                  type="button"
                  className={`${styles.item} ${open ? styles.item_open : ""}`}
                  onClick={() => onOpenSession(st.tip.id)}
                  aria-current={open || undefined}
                >
                  <span className={styles.dot} style={{ background: STATE_DOT[st.state] }} />
                  <span className={styles.item_title}>{sessionTitle(st.tip, untitled)}</span>
                  <span className={styles.item_meta}>{t(`home.section_${st.state}`)}</span>
                  {plan && plan.total > 0 && (
                    <span className={styles.item_meta}>
                      {plan.done}/{plan.total}
                    </span>
                  )}
                </button>
              );
            })}
          </div>
        </section>
      )}

      {steps && steps.length > 0 && (
        <section className={styles.block}>
          <h3 className={styles.head}>
            {t("home.rail_steps", "Steps")}
            <span className={styles.count}>
              {doneSteps}/{steps.length}
            </span>
          </h3>
          <ol className={styles.steps}>
            {steps.map((s, i) => (
              <li
                key={i}
                className={`${styles.step} ${s.done ? styles.step_done : ""} ${i === current ? styles.step_current : ""}`}
              >
                <span className={styles.step_mark} aria-hidden />
                <span className={styles.step_text}>
                  {stepText(s.text) ?? s.text}
                  {i === current && (
                    <span className={styles.step_now}>{t("home.rail_step_now", "(now)")}</span>
                  )}
                </span>
              </li>
            ))}
          </ol>
        </section>
      )}

      {ambient}

      {artifacts && artifacts.length > 0 && (
        <section className={styles.block}>
          <h3 className={styles.head}>
            {t("home.rail_material", "Material")}
            <span className={styles.count}>{artifacts.length}</span>
          </h3>
          <div className={styles.card}>
            {artifacts.map((a) => (
              <button
                key={a.id}
                type="button"
                className={styles.item}
                onClick={() => requestArtifactNav(a.id)}
                title={a.note || a.name}
              >
                <span className={styles.item_icon}>
                  <ArtifactGlyph mime={a.mime} />
                </span>
                <span className={styles.item_title}>{a.title || a.name}</span>
              </button>
            ))}
          </div>
        </section>
      )}

      <section className={styles.block}>
        <button
          type="button"
          className={styles.dev_toggle}
          onClick={() => setDevOpen((v) => !v)}
          aria-expanded={devOpen}
        >
          {t("home.rail_developer", "Developer view")}
          <ChevronRight
            size={13}
            strokeWidth={1.8}
            className={`${styles.dev_chevron} ${devOpen ? styles.dev_chevron_open : ""}`}
          />
        </button>
        {devOpen && <div className={styles.dev}>{developer}</div>}
      </section>
    </aside>
  );
}
