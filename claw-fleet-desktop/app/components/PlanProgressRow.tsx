import type React from "react";
import { useTranslation } from "react-i18next";
import { isKeyboardActivationKey } from "../keyboard";
import type { TaskPlanSummary } from "../generated/types";
import styles from "./PlanProgressRow.module.css";

/** TASKS.md plan progress for one session: plan name, a done/total bar, and the
 *  P-task currently in flight.
 *
 *  Lives in its own component because two places need the same row — the
 *  SessionCard, where it has always been, and the SessionDetail header, where
 *  the plan is the frame for the transcript below it. Rendering it twice from
 *  two copies of the same JSX is how the two drift apart.
 *
 *  `variant` only changes weight and ground, never the content or the ordering:
 *  a reader who learned the row on a card reads the same row in the header. */
interface Props {
  plan: TaskPlanSummary;
  /** Opens the plan — the Tasks tab of the session detail, in both placements. */
  onOpen: (e: React.SyntheticEvent) => void;
  variant?: "card" | "header";
}

/** TASKS.md titles carry `**bold**` markers that mean nothing in a one-line row. */
function stripMd(s: string): string {
  return s.replace(/\*\*/g, "");
}

export function PlanProgressRow({ plan, onOpen, variant = "card" }: Props) {
  const { t } = useTranslation();
  if (plan.total <= 0) return null;

  const donePct = (plan.done / plan.total) * 100;
  const planName = plan.currentPlan ? stripMd(plan.currentPlan) : null;
  const currentTask = plan.currentTask ? stripMd(plan.currentTask) : null;

  return (
    <div
      className={`${styles.row} ${variant === "header" ? styles.header_variant : ""}`}
      title={t("card.tip_task_plan", { done: plan.done, total: plan.total })}
      role="button"
      tabIndex={0}
      onClick={onOpen}
      onKeyDown={(e) => {
        if (!isKeyboardActivationKey(e.key)) return;
        e.preventDefault();
        onOpen(e);
      }}
    >
      <div className={styles.head}>
        <span className={styles.icon} aria-hidden>📋</span>
        {planName && (
          <span className={styles.name} title={planName}>{planName}</span>
        )}
        <div
          className={styles.bar}
          role="progressbar"
          aria-valuenow={plan.done}
          aria-valuemax={plan.total}
        >
          <div className={styles.bar_done} style={{ width: `${donePct}%` }} />
        </div>
        <span className={styles.count}>{plan.done}/{plan.total}</span>
      </div>
      {currentTask && (
        <div className={styles.current} title={currentTask}>{currentTask}</div>
      )}
    </div>
  );
}
