import { useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronDown, ChevronRight } from "lucide-react";
import type { TaskPlanDetail } from "../types";
import { TaskLine } from "./TaskLine";
import styles from "./SessionTasksPanel.module.css";

/**
 * 任务 facet — the TASKS.md plans this session is working, read beside the
 * transcript.
 *
 * It shows the same data as the 计划树, but answers a narrower question: not
 * "what does this repo's plan forest look like" but "where is *this* session
 * up to". So the shape is one plan per block, ordered as the file has them,
 * with the first pending item marked — and everything that isn't that answer
 * pushed down a level:
 *
 *   - Done items fold away behind a count. A 4/5 plan used to spend four fifths
 *     of the panel on struck-through lines nobody was going to read.
 *   - Progress is a meter, not a bare `0/4` — the number is still there for
 *     precision, but the shape is what you read at a glance.
 *   - The plan id and worktree source drop to a meta line under the title;
 *     they are how you *find* the plan, not what you are reading.
 *
 * P-task prose renders through the shared `TaskLine`, which is also what the
 * 计划树 drawer uses — the two surfaces read the same items and had drifted
 * into two different treatments of them.
 */
export function SessionTasksPanel({ plans }: { plans: TaskPlanDetail[] }) {
  return (
    <div className={styles.panel}>
      {plans.map((plan, pi) => (
        <PlanBlock key={plan.id ?? `plan-${pi}`} plan={plan} />
      ))}
    </div>
  );
}

function PlanBlock({ plan }: { plan: TaskPlanDetail }) {
  const { t } = useTranslation();
  const [doneShown, setDoneShown] = useState(false);

  const total = plan.items.length;
  const done = plan.items.filter((it) => it.done).length;
  const allDone = total > 0 && done === total;
  // Prefer the human-readable `**Plan:**` title; fall back to the sentinel id,
  // then to the anonymous label.
  const title = plan.title ?? plan.id ?? t("detail.tasks_anonymous");
  // Keep the id as a secondary tag only when a title is present — otherwise the
  // title already *is* the id, no need to repeat it.
  const showId = Boolean(plan.title && plan.id);
  // The first still-pending item is "current" for this plan — the visible
  // answer to 「做到第几个 P 了」.
  const currentIdx = plan.items.findIndex((it) => !it.done);

  return (
    <section className={styles.plan}>
      <h3 className={styles.title}>{title}</h3>
      <div className={styles.meta}>
        <span
          className={`${styles.status} ${allDone ? styles.status_done : styles.status_active}`}
        >
          {allDone ? t("detail.tasks_status_done") : t("detail.tasks_status_active")}
        </span>
        {plan.kind === "explore" && <span className={styles.kind}>explore</span>}
        {showId && <span className={styles.id}>{plan.id}</span>}
        {plan.source && (
          <span className={styles.source} title={plan.source}>
            {plan.source}
          </span>
        )}
      </div>

      <div className={styles.meter} role="presentation">
        <span className={styles.track}>
          <span
            className={`${styles.fill} ${allDone ? styles.fill_done : ""}`}
            style={{ width: total > 0 ? `${(done / total) * 100}%` : "0%" }}
          />
        </span>
        <span className={styles.count}>
          {done}/{total}
        </span>
      </div>

      <div className={styles.items}>
        {/* Done items collapse behind their count — but only when there is
            something left to show; a finished plan folded to nothing but a
            toggle reads as an empty panel. */}
        {done > 0 && !allDone && (
          <button
            className={styles.fold}
            onClick={() => setDoneShown((v) => !v)}
            aria-expanded={doneShown}
          >
            {doneShown ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
            {t("plans.done_items", { count: done })}
          </button>
        )}
        {plan.items.map((it, ii) => {
          if (it.done && !doneShown && !allDone) return null;
          return (
            <TaskLine
              key={ii}
              text={it.text}
              state={it.done ? "done" : ii === currentIdx ? "current" : "pending"}
            />
          );
        })}
      </div>
    </section>
  );
}
