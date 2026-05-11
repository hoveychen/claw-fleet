// P10 / P12 — Task-as-Unit V1 main view.
//
// Layout:
//   ┌─ TasksView ────────────────────────────────────────────────┐
//   │  [+ New task]  project filter ▾                            │
//   │  ┌─ TaskList ─────┐  ┌─ TaskDetail (kanban) ─────────────┐ │
//   │  │ • Task A  ▶    │  │  ▼ Plan (5 items)   [✎][▶ start]  │ │
//   │  │   Task B  ⏸    │  │  ┌──────────┬──────────┬────────┐ │ │
//   │  │   Task C  ✓    │  │  │ pending  │ running  │  done  │ │ │
//   │  │                │  │  │ p1 ⏳    │ p2 ▶     │ p3 ✓   │ │ │
//   │  │                │  │  └──────────┴──────────┴────────┘ │ │
//   │  └────────────────┘  └────────────────────────────────────┘ │
//   └────────────────────────────────────────────────────────────┘
//
// V1 scope:
// - List tasks (filter by project)
// - Open a task → see its plan items in a 3-column kanban (pending / running / done)
// - "Start task" button transitions a Drafting/Ready task to Running
// - Inbox dialog ("+ New task")
//
// Out of scope for V1 minimum (lands as polish):
// - DAG matrix editor (P14 V2)
// - Inline plan edit (P11 polish)
// - Worker output stream (P10 detail drawer — depends on P7 integration)
// - Human-gate inline approval (P15 — exists via DecisionPanel handoff)

import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import styles from "./TasksView.module.css";
import { InboxDialog } from "./InboxDialog";
import {
  useProjectsStore,
  useTasksStore,
  type Project,
} from "../store";
import type { PItem, PItemStatus, Task, TaskStatus } from "../types";
import { pItemStatusKey } from "../types";

export function TasksView() {
  const { t } = useTranslation();
  const { projects, refresh: refreshProjects } = useProjectsStore();
  const {
    tasks,
    selectedTaskId,
    loaded,
    setSelectedTaskId,
    refresh,
    startTask,
  } = useTasksStore();

  const [filter, setFilter] = useState<string>("");
  const [showInbox, setShowInbox] = useState(false);

  useEffect(() => {
    refreshProjects();
    refresh(filter || undefined);
  }, [refreshProjects, refresh, filter]);

  const visibleTasks = tasks;
  const selected = useMemo(
    () => visibleTasks.find((t) => t.id === selectedTaskId) ?? null,
    [visibleTasks, selectedTaskId],
  );

  return (
    <div className={styles.root}>
      <header className={styles.header}>
        <h1 className={styles.title}>{t("tasks.title", "Tasks")}</h1>
        <div className={styles.header_right}>
          <select
            className={styles.project_filter}
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            aria-label={t("tasks.filter_by_project", "Filter by project")}
          >
            <option value="">{t("tasks.all_projects", "All projects")}</option>
            {projects.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
          </select>
          <button
            className={styles.btn_primary}
            onClick={() => setShowInbox(true)}
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

      <div className={styles.split}>
        <aside className={styles.task_list}>
          {!loaded && <div className={styles.empty}>{t("loading", "Loading…")}</div>}
          {loaded && visibleTasks.length === 0 && (
            <div className={styles.empty}>
              {t("tasks.empty", "No tasks yet — click + New task to start one.")}
            </div>
          )}
          {visibleTasks.map((task) => (
            <TaskListItem
              key={task.id}
              task={task}
              project={projects.find((p) => p.id === task.projectId)}
              active={task.id === selectedTaskId}
              onClick={() => setSelectedTaskId(task.id)}
            />
          ))}
        </aside>

        <section className={styles.detail}>
          {selected ? (
            <TaskDetail task={selected} onStart={() => startTask(selected.id)} />
          ) : (
            <div className={styles.detail_empty}>
              {t(
                "tasks.detail_empty",
                "Pick a task on the left to see its plan.",
              )}
            </div>
          )}
        </section>
      </div>

      {showInbox && (
        <InboxDialog
          projects={projects}
          defaultProjectId={filter || undefined}
          onCancel={() => setShowInbox(false)}
          onCreated={async (task) => {
            setShowInbox(false);
            setSelectedTaskId(task.id);
            await refresh(filter || undefined);
          }}
        />
      )}
    </div>
  );
}

function TaskListItem({
  task,
  project,
  active,
  onClick,
}: {
  task: Task;
  project: Project | undefined;
  active: boolean;
  onClick: () => void;
}) {
  const summary = useMemo(() => summarisePlan(task), [task]);
  return (
    <button
      className={`${styles.task_item} ${active ? styles.task_item_active : ""}`}
      onClick={onClick}
    >
      <div className={styles.task_item_top}>
        <span className={styles.task_item_title}>{task.title}</span>
        <StatusBadge status={task.status} />
      </div>
      <div className={styles.task_item_meta}>
        {project && <span className={styles.project_chip}>{project.name}</span>}
        <span>
          {summary.done}/{summary.total} ✓
        </span>
        {summary.running > 0 && <span>{summary.running} ▶</span>}
        {summary.failed > 0 && (
          <span className={styles.failed_chip}>{summary.failed} ✗</span>
        )}
      </div>
    </button>
  );
}

function TaskDetail({ task, onStart }: { task: Task; onStart: () => void }) {
  const { t } = useTranslation();
  const summary = useMemo(() => summarisePlan(task), [task]);
  const canStart =
    task.status === "drafting" ||
    task.status === "planning" ||
    task.status === "ready";

  const items = Object.values(task.plan.items);

  // Column buckets per PRD §4.5 "Kanban for P-items":
  // - pending: WaitDeps / WaitResource
  // - active: Running / WaitHumanGate
  // - resolved: Done / Skipped / Failed
  const pending: PItem[] = [];
  const active: PItem[] = [];
  const resolved: PItem[] = [];
  for (const p of items) {
    const k = pItemStatusKey(p.status);
    if (k === "waitDeps" || k === "waitResource") pending.push(p);
    else if (k === "running" || k === "waitHumanGate") active.push(p);
    else resolved.push(p);
  }

  // Stable order — id alpha so layout doesn't churn on refetch.
  for (const arr of [pending, active, resolved]) {
    arr.sort((a, b) => a.id.localeCompare(b.id));
  }

  return (
    <div className={styles.task_detail}>
      <div className={styles.task_detail_header}>
        <div>
          <h2 className={styles.task_detail_title}>{task.title}</h2>
          {task.taskBranch && (
            <code className={styles.branch_label}>{task.taskBranch}</code>
          )}
        </div>
        <div className={styles.task_detail_actions}>
          <StatusBadge status={task.status} />
          <button
            className={styles.btn_primary}
            onClick={onStart}
            disabled={!canStart}
            title={
              canStart
                ? ""
                : t("tasks.already_running", "Task is already running or done.")
            }
          >
            ▶ {t("tasks.start", "Start")}
          </button>
        </div>
      </div>

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
  const statusKey = pItemStatusKey(pitem.status);
  const icon = pItemIcon(pitem.status);
  return (
    <div className={`${styles.card} ${styles[`card_${statusKey}`] ?? ""}`}>
      <div className={styles.card_head}>
        <span className={styles.card_icon}>{icon}</span>
        <span className={styles.card_id}>{pitem.id}</span>
        {pitem.humanGate && (
          <span className={styles.gate_chip} title="Human gate">
            👁
          </span>
        )}
      </div>
      <div className={styles.card_desc}>{pitem.desc}</div>
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
    case "waitResource":
      return "⏳";
    case "running":
      return "▶";
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
    else if (k === "running" || k === "waitHumanGate") running += 1;
  }
  return { total, done, failed, running };
}

// Silence unused-import warning when invoke is needed elsewhere.
void invoke;
