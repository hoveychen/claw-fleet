import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import type { SplitFromRef, TaskProfile, TaskProfileBrief, TaskType } from "../generated/types";
import { useSessionsStore, useUIStore } from "../store";
import styles from "./TaskProfileBar.module.css";

const TYPES: TaskType[] = ["dev", "research", "general"];

/** The task's type as a small chip; the goal rides its tooltip. An undecided
 *  task shows no chip: most tasks never get past undecided, so on the list it
 *  would only be noise. */
export function TaskTypeChip({ profile }: { profile: TaskProfileBrief | null | undefined }) {
  const { t } = useTranslation();
  if (!profile || profile.type === "general") return null;
  return (
    <span
      className={styles.type_chip}
      data-type={profile.type}
      data-testid="task-type-chip"
      title={profile.goal ?? undefined}
    >
      {t(`task_profile.type.${profile.type}`)}
    </span>
  );
}

/** The newest main-line hop of task `taskId` in the live scan, or the task id
 *  itself (its first hop) when none of its hops is scanned. */
function taskTip(taskId: string): string {
  let tip: { id: string; createdAtMs: number } | undefined;
  for (const s of useSessionsStore.getState().sessions) {
    if (s.taskId !== taskId || s.parentTaskId) continue;
    if (!tip || s.createdAtMs > tip.createdAtMs) tip = s;
  }
  return tip?.id ?? taskId;
}

/** "Split from <task>": opens the source task on the Tasks page. */
export function SplitFromLink({ splitFrom }: { splitFrom: SplitFromRef }) {
  const { t } = useTranslation();
  const title = splitFrom.title || t("task_profile.split_from_untitled");
  return (
    <button
      type="button"
      className={styles.split_from}
      data-testid="task-split-from"
      title={t("task_profile.split_from_open")}
      onClick={() => useUIStore.getState().requestOpenTask(taskTip(splitFrom.taskId))}
    >
      <span className={styles.split_from_label}>{t("task_profile.split_from")}</span>
      <span className={styles.split_from_title}>{title}</span>
    </button>
  );
}

/**
 * The task page's type + goal line, with an inline editor for the boss.
 * Mirrors core `task_profile::set_by_boss`: replacing an existing goal needs a
 * reason, and the server refuses it without one — the form asks up front so
 * the refusal is rarely what the boss sees.
 */
export function TaskProfileBar({
  sessionId,
  profile,
  splitFrom,
}: {
  sessionId: string;
  profile: TaskProfileBrief | null | undefined;
  /** The task this one was split off from (`fleet__task action=split`). */
  splitFrom?: SplitFromRef | null;
}) {
  const { t } = useTranslation();
  // The last saved value shows at once; the next scan's summary catches up.
  const [saved, setSaved] = useState<TaskProfileBrief | null>(profile ?? null);
  useEffect(() => setSaved(profile ?? null), [profile?.type, profile?.goal, profile?.setBy]);
  const [editing, setEditing] = useState(false);
  const [type, setType] = useState<TaskType>(saved?.type ?? "general");
  const [goal, setGoal] = useState(saved?.goal ?? "");
  const [reason, setReason] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const oldGoal = (saved?.goal ?? "").trim();
  const goalChanged = goal.trim() !== oldGoal && goal.trim() !== "";
  const needsReason = goalChanged && oldGoal !== "";

  const startEdit = () => {
    setType(saved?.type ?? "general");
    setGoal(saved?.goal ?? "");
    setReason("");
    setError(null);
    setEditing(true);
  };

  const save = () => {
    if (needsReason && !reason.trim()) {
      setError(t("task_profile.reason_required"));
      return;
    }
    const typeChanged = type !== (saved?.type ?? "general") || !saved;
    if (!typeChanged && !goalChanged) {
      setEditing(false);
      return;
    }
    setBusy(true);
    invoke<TaskProfile>("set_task_profile", {
      sessionId,
      taskType: typeChanged ? type : null,
      goal: goalChanged ? goal.trim() : null,
      goalReason: needsReason ? reason.trim() : null,
    })
      .then((p) => {
        setSaved({ type: p.type, goal: p.goal ?? null, setBy: p.setBy });
        setEditing(false);
      })
      .catch((e) => setError(String(e)))
      .finally(() => setBusy(false));
  };

  if (!editing) {
    return (
      <div className={styles.bar} data-testid="task-profile-bar">
        <span className={styles.type_chip} data-type={saved?.type ?? "general"}>
          {t(`task_profile.type.${saved?.type ?? "general"}`)}
        </span>
        <span className={saved?.goal ? styles.goal : styles.goal_empty} title={saved?.goal ?? undefined}>
          {saved?.goal || t("task_profile.no_goal")}
        </span>
        {saved?.setBy === "boss" && <span className={styles.by_boss}>{t("task_profile.set_by_boss")}</span>}
        <button type="button" className={styles.edit_btn} onClick={startEdit}>
          {t("task_profile.edit")}
        </button>
        {splitFrom && <SplitFromLink splitFrom={splitFrom} />}
      </div>
    );
  }

  return (
    <div className={styles.editor} data-testid="task-profile-editor">
      <div className={styles.field}>
        <span className={styles.label}>{t("task_profile.type_label")}</span>
        <div className={styles.seg} role="radiogroup">
          {TYPES.map((ty) => (
            <button
              key={ty}
              type="button"
              role="radio"
              aria-checked={type === ty}
              className={`${styles.seg_btn} ${type === ty ? styles.seg_btn_on : ""}`}
              onClick={() => setType(ty)}
            >
              {t(`task_profile.type.${ty}`)}
            </button>
          ))}
        </div>
      </div>
      <label className={styles.field}>
        <span className={styles.label}>{t("task_profile.goal_label")}</span>
        <textarea
          className={styles.input}
          rows={2}
          value={goal}
          placeholder={t("task_profile.goal_placeholder")}
          onChange={(e) => setGoal(e.target.value)}
        />
      </label>
      {needsReason && (
        <label className={styles.field}>
          <span className={styles.label}>{t("task_profile.reason_label")}</span>
          <input
            className={styles.input}
            value={reason}
            placeholder={t("task_profile.reason_placeholder")}
            onChange={(e) => setReason(e.target.value)}
          />
        </label>
      )}
      {error && <div className={styles.error}>{error}</div>}
      <div className={styles.actions}>
        <button type="button" className={styles.cancel_btn} onClick={() => setEditing(false)} disabled={busy}>
          {t("task_profile.cancel")}
        </button>
        <button type="button" className={styles.save_btn} onClick={save} disabled={busy}>
          {t("task_profile.save")}
        </button>
      </div>
    </div>
  );
}
