// Task-as-Unit V2 — single-column tasks view.
//
// Two states, gated on selectedTaskId:
//   - selectedTaskId == null → render the centered TaskComposer (the default
//     new-task entry point). The flat task list lives in the sidebar now.
//   - selectedTaskId != null → render the task detail (来龙去脉 + DAG +
//     deliverables + metrics), back button clears the selection.

import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import styles from "./TasksView.module.css";
import { TaskComposer } from "./TaskComposer";
import {
  useProjectsStore,
  useTasksStore,
  type Project,
} from "../store";
import { useRuntimeTasksStore } from "../runtimeTasksStore";
import type { PItem, PItemStatus, Task, TaskStatus } from "../types";
import { pItemStatusKey } from "../types";
import { PROJECTS_FEATURE_ENABLED } from "../featureFlags";

export function TasksView() {
  if (!PROJECTS_FEATURE_ENABLED) return null;
  const { projects, refresh: refreshProjects } = useProjectsStore();
  const selectedProjectId = useProjectsStore((s) => s.selectedProjectId);
  const {
    tasks,
    selectedTaskId,
    setSelectedTaskId,
    refresh,
    startTask,
    acceptTask,
    updateTaskTitle,
  } = useTasksStore();

  useEffect(() => {
    refreshProjects();
    refresh(selectedProjectId ?? undefined);
  }, [refreshProjects, refresh, selectedProjectId]);

  const selectedTask = useMemo(
    () => tasks.find((tk) => tk.id === selectedTaskId) ?? null,
    [tasks, selectedTaskId],
  );

  // Detail view (P-item kanban for the selected task).
  if (selectedTask) {
    const taskProject = projects.find((p) => p.id === selectedTask.projectId) ?? null;
    return (
      <div className={styles.root}>
        <TaskDetailHeader
          task={selectedTask}
          project={taskProject}
          onBack={() => setSelectedTaskId(null)}
          onStart={() => startTask(selectedTask.id)}
          onAccept={() => acceptTask(selectedTask.id)}
          onRename={(title) => updateTaskTitle(selectedTask.id, title)}
        />
        <TaskDetailBody task={selectedTask} />
      </div>
    );
  }

  // Default — the centered composer. The task list is in the sidebar.
  return (
    <div className={styles.root}>
      <TaskComposer
        defaultProjectId={selectedProjectId}
        onCreated={async (task) => {
          setSelectedTaskId(task.id);
          await refresh(selectedProjectId ?? undefined);
        }}
      />
    </div>
  );
}

function LiveDot() {
  return (
    <span
      title="Backed by a live fleet-task process"
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: 4,
        fontSize: 10,
        color: "var(--text-muted, #888)",
        marginLeft: 4,
      }}
    >
      <span
        style={{
          width: 6,
          height: 6,
          borderRadius: 999,
          background: "#22c55e",
          boxShadow: "0 0 6px #22c55e80",
        }}
      />
      live
    </span>
  );
}

function TaskDetailHeader({
  task,
  project,
  onBack,
  onStart,
  onAccept,
  onRename,
}: {
  task: Task;
  project: Project | null;
  onBack: () => void;
  onStart: () => Promise<void>;
  onAccept: () => Promise<void>;
  onRename: (title: string) => Promise<void>;
}) {
  const { t } = useTranslation();
  const [starting, setStarting] = useState(false);
  const [startError, setStartError] = useState<string | null>(null);
  const [accepting, setAccepting] = useState(false);
  const canStart = !starting && task.status === "drafting";
  const canAccept = !accepting && task.status === "awaitingAcceptance";
  const handleStart = async () => {
    setStarting(true);
    setStartError(null);
    try {
      await onStart();
    } catch (e) {
      setStartError(String((e as { message?: string })?.message ?? e));
    } finally {
      setStarting(false);
    }
  };
  const handleAccept = async () => {
    setAccepting(true);
    setStartError(null);
    try {
      await onAccept();
    } catch (e) {
      setStartError(String((e as { message?: string })?.message ?? e));
    } finally {
      setAccepting(false);
    }
  };
  return (
    <>
      <div className={styles.detail_header}>
        <div className={styles.detail_header_left}>
          <button
            className={styles.back_btn}
            onClick={onBack}
            title={t("tasks.back_to_list", "Back to task list")}
            aria-label={t("tasks.back_to_list", "Back to task list")}
          >
            ←
          </button>
          <div>
            <div className={styles.detail_breadcrumb}>
              {project?.name ?? t("tasks.unknown_project", "Unknown project")}
            </div>
            <EditableTaskTitle task={task} onRename={onRename} />
          </div>
        </div>
        <div className={styles.detail_header_right}>
          <StatusBadge status={task.status} />
          {useRuntimeTasksStore.getState().byTaskId[task.id] && <LiveDot />}
          {task.status === "awaitingAcceptance" ? (
            <button
              className={styles.btn_primary}
              onClick={handleAccept}
              disabled={!canAccept}
              title={t("tasks.accept_hint", "All work reviewed — accept to mark the task done.")}
            >
              {accepting
                ? t("tasks.accepting", "Accepting…")
                : `✓ ${t("tasks.accept", "Accept")}`}
            </button>
          ) : (
            <button
              className={styles.btn_primary}
              onClick={handleStart}
              disabled={!canStart}
              title={
                canStart
                  ? ""
                  : t("tasks.already_running", "Task is already running or done.")
              }
            >
              {starting
                ? t("tasks.starting", "Starting…")
                : `▶ ${t("tasks.start", "Start")}`}
            </button>
          )}
        </div>
      </div>
      {startError && (
        <div className={styles.error}>
          {t("tasks.start_failed", "Start failed: {{message}}", {
            message: startError,
          })}
        </div>
      )}
    </>
  );
}

function EditableTaskTitle({
  task,
  onRename,
}: {
  task: Task;
  onRename: (title: string) => Promise<void>;
}) {
  const { t } = useTranslation();
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(task.title);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!editing) setDraft(task.title);
  }, [task.title, editing]);

  useEffect(() => {
    if (editing) {
      inputRef.current?.focus();
      inputRef.current?.select();
    }
  }, [editing]);

  const commit = async () => {
    const next = draft.trim();
    if (!next || next === task.title) {
      setEditing(false);
      setDraft(task.title);
      return;
    }
    try {
      await onRename(next);
    } catch {
      setDraft(task.title);
    } finally {
      setEditing(false);
    }
  };

  if (editing) {
    return (
      <input
        ref={inputRef}
        className={styles.task_detail_title_input}
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            void commit();
          } else if (e.key === "Escape") {
            e.preventDefault();
            setDraft(task.title);
            setEditing(false);
          }
        }}
      />
    );
  }

  return (
    <h2
      className={styles.task_detail_title}
      onClick={() => setEditing(true)}
      title={t("tasks.click_to_rename", "Click to rename")}
    >
      {task.title}
      {task.titleAuto && (
        <span
          className={styles.title_auto_chip}
          title={t(
            "tasks.title_auto_hint",
            "Auto-generated title — will be replaced by the AI summary, or click to override.",
          )}
        >
          ✎
        </span>
      )}
    </h2>
  );
}

function TaskDetailBody({ task }: { task: Task }) {
  const { t } = useTranslation();
  const summary = useMemo(() => summarisePlan(task), [task]);
  const items = Object.values(task.plan.items);

  // Column buckets per PRD §4.5 "Kanban for P-items":
  // - pending: WaitDeps
  // - active: Running / Reviewing / WaitHumanGate
  // - resolved: Done / Skipped / Failed
  const pending: PItem[] = [];
  const active: PItem[] = [];
  const resolved: PItem[] = [];
  for (const p of items) {
    const k = pItemStatusKey(p.status);
    if (k === "waitDeps") pending.push(p);
    else if (k === "running" || k === "reviewing" || k === "waitHumanGate") active.push(p);
    else resolved.push(p);
  }
  for (const arr of [pending, active, resolved]) {
    arr.sort((a, b) => a.id.localeCompare(b.id));
  }

  return (
    <div className={styles.task_detail}>
      {task.taskBranch && (
        <code className={styles.branch_label}>{task.taskBranch}</code>
      )}
      {task.description && (
        <p className={styles.task_description}>{task.description}</p>
      )}

      {items.length === 0 && (
        <div className={styles.no_plan}>
          {task.status === "running"
            ? t(
                "tasks.planning_in_progress",
                "Planning session is clarifying the requirements with you — answer its decision cards; the plan appears here once it's written.",
              )
            : t(
                "tasks.no_plan_yet",
                "No plan yet. Start the task to launch the interactive planning session.",
              )}
        </div>
      )}

      {items.length > 0 && (
        <div className={styles.kanban}>
          <KanbanColumn
            title={t("tasks.col_pending", "Pending")}
            count={pending.length}
            items={pending}
          />
          <KanbanColumn
            title={t("tasks.col_active", "Active")}
            count={active.length}
            items={active}
            highlight
          />
          <KanbanColumn
            title={t("tasks.col_resolved", "Resolved")}
            count={resolved.length}
            items={resolved}
          />
        </div>
      )}

      <footer className={styles.task_detail_footer}>
        <span>
          {summary.done} / {summary.total} {t("tasks.complete", "complete")}
        </span>
        {summary.failed > 0 && (
          <span className={styles.failed}>
            {summary.failed} {t("tasks.failed", "failed")}
          </span>
        )}
      </footer>
    </div>
  );
}

function KanbanColumn({
  title,
  count,
  items,
  highlight = false,
}: {
  title: string;
  count: number;
  items: PItem[];
  highlight?: boolean;
}) {
  return (
    <div className={`${styles.column} ${highlight ? styles.column_active : ""}`}>
      <div className={styles.column_header}>
        <span>{title}</span>
        <span className={styles.column_count}>{count}</span>
      </div>
      <div className={styles.column_body}>
        {items.length === 0 && <div className={styles.col_empty}>—</div>}
        {items.map((p) => (
          <PItemCard key={p.id} pitem={p} />
        ))}
      </div>
    </div>
  );
}

function PItemCard({ pitem }: { pitem: PItem }) {
  const { t } = useTranslation();
  const statusKey = pItemStatusKey(pitem.status);
  const icon = pItemIcon(pitem.status);
  return (
    <div className={`${styles.card} ${styles[`card_${statusKey}`] ?? ""}`}>
      <div className={styles.card_head}>
        <span className={styles.card_icon}>{icon}</span>
        <span className={styles.card_id}>{pitem.id}</span>
        {pitem.humanGate && (
          <span className={styles.gate_chip} title={t("tasks.human_gate_required", "Human gate required")}>
            👁
          </span>
        )}
      </div>
      <div className={styles.card_desc}>{pitem.desc}</div>
      {statusKey === "waitHumanGate" && (
        <div className={styles.gate_pending}>
          {t("tasks.waiting_for_user_review", "等待用户审核 — 请在 Decision Panel 处理")}
        </div>
      )}
      {pitem.touches.length > 0 && (
        <div className={styles.card_touches}>
          {pitem.touches.slice(0, 3).map((t) => (
            <code key={t}>{t}</code>
          ))}
          {pitem.touches.length > 3 && (
            <span>+{pitem.touches.length - 3}</span>
          )}
        </div>
      )}
    </div>
  );
}

function StatusBadge({ status }: { status: TaskStatus }) {
  const cls = `${styles.status_badge} ${styles[`status_${status}`] ?? ""}`;
  return <span className={cls}>{status}</span>;
}

function pItemIcon(status: PItemStatus): string {
  const key = pItemStatusKey(status);
  switch (key) {
    case "waitDeps":
      return "🔒";
    case "running":
      return "▶";
    case "reviewing":
      return "🔍";
    case "waitHumanGate":
      return "👁";
    case "done":
      return "✓";
    case "skipped":
      return "⤼";
    case "failed":
      return "✗";
    default:
      return "•";
  }
}

function summarisePlan(task: Task) {
  let total = 0;
  let done = 0;
  let failed = 0;
  let running = 0;
  for (const p of Object.values(task.plan.items)) {
    total += 1;
    const k = pItemStatusKey(p.status);
    if (k === "done") done += 1;
    else if (k === "failed") failed += 1;
    else if (k === "running" || k === "reviewing" || k === "waitHumanGate") running += 1;
  }
  return { total, done, failed, running };
}
