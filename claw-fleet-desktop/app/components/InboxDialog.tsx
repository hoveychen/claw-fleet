// P11 — Inbox dialog: the entry point for creating a new task.
//
// Users pick a project, type a title + description, drop optional materials
// (files / text). Hitting "Start drafting" creates the task in `Drafting`
// status; the user can then run the planner / edit the plan / start it.
//
// V1 minimum: title + description + project picker only. File drop and
// planner-button arrive later (file upload command + atomic-plan-tasks skill
// invocation are downstream P-items). Hooks for those slots are reserved in
// the layout so the V2 add is a swap, not a rewrite.

import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import styles from "./InboxDialog.module.css";
import type { Project } from "../store";
import type { Task, TaskInput } from "../types";

interface Props {
  projects: Project[];
  defaultProjectId?: string;
  onCreated: (task: Task) => void;
  onCancel: () => void;
}

export function InboxDialog({ projects, defaultProjectId, onCreated, onCancel }: Props) {
  const { t } = useTranslation();
  const [projectId, setProjectId] = useState(
    defaultProjectId ?? projects[0]?.id ?? "",
  );
  const [title, setTitle] = useState("");
  const [description, setDescription] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const titleRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    titleRef.current?.focus();
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !submitting) onCancel();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onCancel, submitting]);

  const canSubmit = projectId && title.trim().length > 0 && !submitting;

  const submit = async () => {
    if (!canSubmit) return;
    setSubmitting(true);
    setError(null);
    try {
      const input: TaskInput = {
        projectId,
        title: title.trim(),
        description: description.trim(),
      };
      const task = await invoke<Task>("create_task", { input });
      onCreated(task);
    } catch (e) {
      setError(String(e));
      setSubmitting(false);
    }
  };

  return (
    <div className={styles.overlay} onClick={() => !submitting && onCancel()}>
      <div className={styles.dialog} onClick={(e) => e.stopPropagation()}>
        <h2 className={styles.title}>{t("inbox.title", "New task")}</h2>

        {projects.length > 1 && (
          <label className={styles.field}>
            <span className={styles.label}>{t("inbox.project", "Project")}</span>
            <select
              className={styles.select}
              value={projectId}
              onChange={(e) => setProjectId(e.target.value)}
              disabled={submitting}
            >
              {projects.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                </option>
              ))}
            </select>
          </label>
        )}

        <label className={styles.field}>
          <span className={styles.label}>{t("inbox.task_title", "Title")}</span>
          <input
            ref={titleRef}
            className={styles.input}
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            placeholder={t("inbox.title_placeholder", "Add bookmarks UI")}
            disabled={submitting}
          />
        </label>

        <label className={styles.field}>
          <span className={styles.label}>
            {t("inbox.description", "What needs to happen?")}
          </span>
          <textarea
            className={styles.textarea}
            rows={5}
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            placeholder={t(
              "inbox.description_placeholder",
              "Describe the task. The planner uses this to draft the plan.",
            )}
            disabled={submitting}
          />
        </label>

        <div className={styles.materials_slot}>
          {/* V2: drag-drop file / paste-image surface lands here. */}
          <span className={styles.materials_hint}>
            {t(
              "inbox.materials_v2",
              "File / screenshot upload arrives in a follow-up.",
            )}
          </span>
        </div>

        {error && <div className={styles.error}>{error}</div>}

        <div className={styles.actions}>
          <button
            className={styles.btn}
            onClick={onCancel}
            disabled={submitting}
          >
            {t("cancel", "Cancel")}
          </button>
          <button
            className={`${styles.btn} ${styles.btn_primary}`}
            onClick={submit}
            disabled={!canSubmit}
          >
            {submitting
              ? t("inbox.creating", "Creating…")
              : t("inbox.create", "Create task")}
          </button>
        </div>
      </div>
    </div>
  );
}
