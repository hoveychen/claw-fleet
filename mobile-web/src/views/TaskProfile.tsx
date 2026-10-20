// A task's type and goal on the phone: a chip on the task row, and a readout
// with an inline editor in the session detail sheet. Same contract as the
// desktop `TaskProfileBar`: replacing an existing goal needs a reason, and the
// host (`task_profile::set_by_boss`) refuses one without it — the form asks up
// front so the refusal is rarely what the boss sees.

import { useEffect, useState } from "react";
import { t } from "../i18n";
import type { TaskProfile, TaskProfileBrief, TaskType } from "../generated/types";
import type { FleetTransport } from "../transport";
import { Spinner } from "./loading";
import styles from "./TaskProfile.module.css";

const TYPES: TaskType[] = ["dev", "research", "general"];

export function taskTypeLabel(type: TaskType): string {
  switch (type) {
    case "dev":
      return t("开发");
    case "research":
      return t("调研");
    default:
      return t("通用");
  }
}

/** The task's type as a small chip; the goal rides its tooltip. */
export function TaskTypeChip({ profile }: { profile: TaskProfileBrief | null | undefined }) {
  if (!profile) return null;
  return (
    <span
      className={styles.chip}
      data-type={profile.type}
      data-testid="task-type-chip"
      title={profile.goal ?? undefined}
    >
      {taskTypeLabel(profile.type)}
    </span>
  );
}

export function TaskProfileSection({
  sessionId,
  profile,
  client,
}: {
  sessionId: string;
  profile: TaskProfileBrief | null | undefined;
  /** `null` when the device is unreachable: the readout stays, editing hides. */
  client: FleetTransport | null;
}) {
  // The last saved value shows at once; the next snapshot catches up.
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
    if (!client) return;
    if (needsReason && !reason.trim()) {
      setError(t("改目标要写一句理由"));
      return;
    }
    const typeChanged = !saved || type !== saved.type;
    if (!typeChanged && !goalChanged) {
      setEditing(false);
      return;
    }
    const params: Record<string, unknown> = { sessionId };
    if (typeChanged) params.type = type;
    if (goalChanged) params.goal = goal.trim();
    if (needsReason) params.goalReason = reason.trim();
    setBusy(true);
    client
      .request<TaskProfile>("task_profile_set", params)
      .then((p) => {
        setSaved({ type: p.type, goal: p.goal ?? null, setBy: p.setBy });
        setEditing(false);
      })
      .catch((e) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setBusy(false));
  };

  if (!editing) {
    return (
      <div className={styles.readout} data-testid="task-profile">
        <div className={styles.readoutHead}>
          <span className={styles.chip} data-type={saved?.type ?? "general"}>
            {taskTypeLabel(saved?.type ?? "general")}
          </span>
          {saved?.setBy === "boss" && <span className={styles.byBoss}>{t("你改过")}</span>}
          {client && (
            <button type="button" className={styles.editBtn} onClick={startEdit}>
              {t("改写")}
            </button>
          )}
        </div>
        <div className={saved?.goal ? styles.goal : styles.goalEmpty}>
          {saved?.goal || t("还没定目标")}
        </div>
      </div>
    );
  }

  return (
    <div className={styles.editor} data-testid="task-profile-editor">
      <span className={styles.label}>{t("类型")}</span>
      <div className={styles.seg} role="radiogroup">
        {TYPES.map((ty) => (
          <button
            key={ty}
            type="button"
            role="radio"
            aria-checked={type === ty}
            className={styles.segBtn}
            data-on={type === ty || undefined}
            onClick={() => setType(ty)}
          >
            {taskTypeLabel(ty)}
          </button>
        ))}
      </div>
      <label className={styles.field}>
        <span className={styles.label}>{t("目标")}</span>
        <textarea
          className={styles.input}
          rows={3}
          value={goal}
          placeholder={t("一句话：做完什么这件事才算完")}
          onChange={(e) => setGoal(e.target.value)}
        />
      </label>
      {needsReason && (
        <label className={styles.field}>
          <span className={styles.label}>{t("为什么改目标")}</span>
          <input
            className={styles.input}
            value={reason}
            placeholder={t("会记进目标历史")}
            onChange={(e) => setReason(e.target.value)}
          />
        </label>
      )}
      {error && <div className={styles.error}>{error}</div>}
      <div className={styles.actions}>
        <button
          type="button"
          className={styles.cancelBtn}
          onClick={() => setEditing(false)}
          disabled={busy}
        >
          {t("取消")}
        </button>
        <button type="button" className={styles.saveBtn} onClick={save} disabled={busy}>
          {busy ? <Spinner size={12} /> : t("保存")}
        </button>
      </div>
    </div>
  );
}
