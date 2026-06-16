// Task-as-Unit V2 — single-column tasks view.
//
// Two phases, gated on selectedTaskId:
//   - selectedTaskId == null → render a card list of tasks scoped by the
//     sidebar's selectedProjectId (the project entry was clicked).
//   - selectedTaskId != null → render TaskDetailView for that task
//     (P-item Kanban, back button to clear selection).
//
// Creation entry is the sidebar's per-project `+` button, which fires an
// `inboxRequest` carrying the projectId; this view consumes the request,
// pops the InboxDialog with the projectId pre-filled, and selects the new
// task afterwards (lands you on the detail phase).

import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Play, Pause, Circle, ListTodo } from "lucide-react";
import styles from "./TasksView.module.css";
import { InboxDialog } from "./InboxDialog";
import { EmptyState } from "./EmptyState";
import {
  useProjectsStore,
  useTasksStore,
  useUIStore,
  type Project,
} from "../store";
import { useRuntimeTasksStore } from "../runtimeTasksStore";
import type { PItem, PItemStatus, Task, TaskStatus } from "../types";
import { pItemStatusKey } from "../types";
import { PROJECTS_FEATURE_ENABLED } from "../featureFlags";

export function TasksView() {
  if (!PROJECTS_FEATURE_ENABLED) return null;
  const { t } = useTranslation();
  const { projects, refresh: refreshProjects } = useProjectsStore();
  const selectedProjectId = useProjectsStore((s) => s.selectedProjectId);
  const {
    tasks,
    selectedTaskId,
    loaded,
    setSelectedTaskId,
    refresh,
    startTask,
    updateTaskTitle,
  } = useTasksStore();

  const [showInbox, setShowInbox] = useState(false);
  const [pendingInboxProjectId, setPendingInboxProjectId] = useState<string | null>(null);
  const inboxRequest = useUIStore((s) => s.inboxRequest);
  const clearInboxRequest = useUIStore((s) => s.clearInboxRequest);

  useEffect(() => {
    refreshProjects();
    refresh(selectedProjectId ?? undefined);
  }, [refreshProjects, refresh, selectedProjectId]);

  // Sidebar's per-project `+` → opens InboxDialog with projectId pre-filled.
  useEffect(() => {
    if (inboxRequest) {
      setPendingInboxProjectId(inboxRequest.projectId);
      setShowInbox(true);
      clearInboxRequest();
    }
  }, [inboxRequest, clearInboxRequest]);

  const project = useMemo(
    () => projects.find((p) => p.id === selectedProjectId) ?? null,
    [projects, selectedProjectId],
  );

  const projectTasks = useMemo(
    () => (selectedProjectId ? tasks.filter((tk) => tk.projectId === selectedProjectId) : tasks),
    [tasks, selectedProjectId],
  );

  const selectedTask = useMemo(
    () => tasks.find((tk) => tk.id === selectedTaskId) ?? null,
    [tasks, selectedTaskId],
  );

  // Phase 2 — detail view (P-item kanban for the selected task).
  if (selectedTask) {
    const taskProject = projects.find((p) => p.id === selectedTask.projectId) ?? null;
    return (
      <div className={styles.root}>
        <TaskDetailHeader
          task={selectedTask}
          project={taskProject}
          onBack={() => setSelectedTaskId(null)}
          onStart={() => startTask(selectedTask.id)}
          onRename={(title) => updateTaskTitle(selectedTask.id, title)}
        />
        <TaskDetailBody task={selectedTask} />
        {showInbox && (
          <InboxDialog
            projects={projects}
            defaultProjectId={pendingInboxProjectId ?? selectedProjectId ?? undefined}
            onCancel={() => setShowInbox(false)}
            onCreated={async (task) => {
              setShowInbox(false);
              setPendingInboxProjectId(null);
              setSelectedTaskId(task.id);
              await refresh(selectedProjectId ?? undefined);
            }}
          />
        )}
      </div>
    );
  }

  // Phase 1 — card list scoped to selectedProjectId.
  const runningCount = projectTasks.filter((tk) => tk.status === "running").length;

  return (
    <div className={styles.root}>
      <header className={styles.header}>
        <div className={styles.header_titles}>
          <h1 className={styles.title}>
            {project ? project.name : t("tasks.all_projects", "All projects")}
          </h1>
          <div className={styles.subtitle}>
            {t("tasks.count_summary", "{{total}} tasks · {{running}} running", {
              total: projectTasks.length,
              running: runningCount,
            })}
          </div>
        </div>
        <div className={styles.header_right}>
          <button
            className={styles.btn_primary}
            onClick={() => {
              setPendingInboxProjectId(selectedProjectId);
              setShowInbox(true);
            }}
            disabled={projects.length === 0}
            title={
              projects.length === 0
                ? t("tasks.create_project_first", "Create a project first")
                : ""
            }
          >
            + {t("tasks.new_task", "New task")}
          </button>
        </div>
      </header>

      <div className={styles.card_list}>
        {!loaded && <div className={styles.empty}>{t("loading", "Loading…")}</div>}
        {loaded && projectTasks.length === 0 && (
          <EmptyState
            icon={<ListTodo size={28} strokeWidth={1.5} />}
            title={t("empty_state.tasks_title")}
            subtitle={t("empty_state.tasks_subtitle")}
          />
        )}
        {projectTasks.map((task) => (
          <TaskCard
            key={task.id}
            task={task}
            project={projects.find((p) => p.id === task.projectId)}
            showProjectChip={!selectedProjectId}
            onClick={() => setSelectedTaskId(task.id)}
          />
        ))}
      </div>

      {showInbox && (
        <InboxDialog
          projects={projects}
          defaultProjectId={pendingInboxProjectId ?? selectedProjectId ?? undefined}
          onCancel={() => setShowInbox(false)}
          onCreated={async (task) => {
            setShowInbox(false);
            setPendingInboxProjectId(null);
            setSelectedTaskId(task.id);
            await refresh(selectedProjectId ?? undefined);
          }}
        />
      )}
    </div>
  );
}

function TaskCard({
  task,
  project,
  showProjectChip,
  onClick,
}: {
  task: Task;
  project: Project | undefined;
  showProjectChip: boolean;
  onClick: () => void;
}) {
  const summary = useMemo(() => summarisePlan(task), [task]);
  const isLive = useRuntimeTasksStore((s) => Boolean(s.byTaskId[task.id]));
  const icon: ReactNode =
    task.status === "running" ? <Play size={11} strokeWidth={1.75} /> :
    task.status === "paused" ? <Pause size={11} strokeWidth={1.75} /> :
    task.status === "reviewing" ? <Circle size={11} strokeWidth={1.75} /> :
    task.status === "awaitingAcceptance" ? <Circle size={11} strokeWidth={1.75} /> :
    task.status === "drafting" ? "✎" :
    task.status === "done" ? "✓" :
    task.status === "abandoned" ? "⤼" :
    "•";
  return (
    <button className={styles.card_row} onClick={onClick}>
      <div className={styles.card_row_top}>
        <span className={styles.card_row_icon}>{icon}</span>
        <span className={styles.card_row_title}>{task.title}</span>
        <StatusBadge status={task.status} />
        {isLive && <LiveDot />}
      </div>
      {task.description && (
        <div className={styles.card_row_desc}>{task.description}</div>
      )}
      <div className={styles.card_row_meta}>
        {showProjectChip && project && (
          <span className={styles.project_chip}>{project.name}</span>
        )}
        <span>
          {summary.done}/{summary.total} ✓
        </span>
        {summary.running > 0 && <span>{summary.running} ▶</span>}
        {summary.failed > 0 && (
          <span className={styles.failed_chip}>{summary.failed} ✗</span>
        )}
        {task.taskBranch && (
          <code className={styles.branch_label}>{task.taskBranch}</code>
        )}
      </div>
    </button>
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
  onRename,
}: {
  task: Task;
  project: Project | null;
  onBack: () => void;
  onStart: () => Promise<void>;
  onRename: (title: string) => Promise<void>;
}) {
  const { t } = useTranslation();
  const [starting, setStarting] = useState(false);
  const [startError, setStartError] = useState<string | null>(null);
  const canStart = !starting && task.status === "drafting";
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
          {t(
            "tasks.no_plan_yet",
            "No plan yet. The planner runs once you draft the task description and attach materials.",
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
