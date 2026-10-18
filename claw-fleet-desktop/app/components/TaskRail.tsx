// The right column of a task: its sub-tasks, its steps, the material it
// produced, and — folded away under "developer view" — the session internals
// (tokens, notes, temp files, background commands, workflow, relay chain).

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

export function TaskRail({
  row,
  openSessionId,
  steps,
  onOpenSession,
  developer,
}: {
  row: TaskRow;
  /** The session the conversation column shows: the main line, or a sub-task. */
  openSessionId: string;
  /** The task's plan items, or null when it has no plan. */
  steps: TaskItem[] | null;
  onOpenSession: (sessionId: string) => void;
  /** The developer view's content, built by the conversation pane that owns
   *  the facets it opens. */
  developer: ReactNode;
}) {
  const { t } = useTranslation();
  const untitled = t("history.untitled");
  const requestArtifactNav = useUIStore((s) => s.requestArtifactNav);
  const [devOpen, setDevOpen] = useState(false);

  const inSubtask = !row.hops.some((h) => h.id === openSessionId);

  // Material is whatever the task's sessions filed in the artifact library.
  const [artifacts, setArtifacts] = useState<Artifact[] | null>(null);
  const memberKey = useMemo(() => [...rowMemberIds(row)].sort().join(","), [row]);
  useEffect(() => {
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

  const doneSubtasks = row.subtasks.filter(
    (st) => st.state === "review" || st.state === "done",
  ).length;
  const doneSteps = steps?.filter((s) => s.done).length ?? 0;
  const current = steps?.findIndex((s) => !s.done) ?? -1;

  return (
    <aside className={styles.rail} data-testid="task-rail">
      {(row.subtasks.length > 0 || inSubtask) && (
        <section className={styles.block}>
          <h3 className={styles.head}>
            {t("home.rail_subtasks", "Sub-tasks")}
            <span className={styles.count}>
              {doneSubtasks}/{row.subtasks.length}
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
            {row.subtasks.map((st) => {
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

      <section className={styles.block}>
        <h3 className={styles.head}>
          {t("home.rail_material", "Material")}
          {artifacts && artifacts.length > 0 && (
            <span className={styles.count}>{artifacts.length}</span>
          )}
        </h3>
        {artifacts && artifacts.length > 0 ? (
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
        ) : (
          <p className={styles.empty}>
            {artifacts == null ? "…" : t("home.rail_material_empty", "Nothing filed yet")}
          </p>
        )}
      </section>

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
