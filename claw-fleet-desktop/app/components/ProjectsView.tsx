import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  useProjectsStore,
  useTasksStore,
  useUIStore,
  type KanbanColumn,
  type Project,
} from "../store";
import { FolderGit2, Search } from "lucide-react";
import { EmptyState } from "./EmptyState";
import styles from "./ProjectsView.module.css";
import { PROJECTS_FEATURE_ENABLED } from "../featureFlags";
import type { TaskStatus } from "../types";

const STATUS_FILTERS: ("all" | TaskStatus)[] = [
  "all",
  "running",
  "reviewing",
  "awaitingAcceptance",
  "paused",
  "drafting",
  "done",
  "abandoned",
];

type DialogMode =
  | { type: "create" }
  | { type: "edit"; project: Project }
  | { type: "delete"; project: Project }
  | null;

export function ProjectsView() {
  if (!PROJECTS_FEATURE_ENABLED) return null;
  const { t } = useTranslation();
  const projects = useProjectsStore((s) => s.projects);
  const fleetSessions = useProjectsStore((s) => s.fleetSessions);
  const loaded = useProjectsStore((s) => s.loaded);
  const selectedId = useProjectsStore((s) => s.selectedProjectId);
  const setSelectedId = useProjectsStore((s) => s.setSelectedProjectId);
  const [dialog, setDialog] = useState<DialogMode>(null);
  const [error, setError] = useState<string | null>(null);
  const showNewProjectRequested = useUIStore((s) => s.showNewProjectRequested);
  const setShowNewProjectRequested = useUIStore((s) => s.setShowNewProjectRequested);

  const load = useCallback(async () => {
    await useProjectsStore.getState().refresh();
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  // Sidebar "+ New project" CTA hands off via a transient store flag.
  useEffect(() => {
    if (showNewProjectRequested) {
      setError(null);
      setDialog({ type: "create" });
      setShowNewProjectRequested(false);
    }
  }, [showNewProjectRequested, setShowNewProjectRequested]);

  const selected = useMemo(
    () => projects.find((p) => p.id === selectedId) ?? null,
    [projects, selectedId],
  );

  const sessionCountByProject = useMemo(() => {
    const map = new Map<string, number>();
    for (const s of fleetSessions) {
      map.set(s.projectId, (map.get(s.projectId) ?? 0) + 1);
    }
    return map;
  }, [fleetSessions]);

  return (
    <div className={styles.page}>
      {!loaded ? (
        <p className={styles.empty}>{t("scanning")}</p>
      ) : projects.length === 0 ? (
        <EmptyState
          icon={<FolderGit2 size={28} strokeWidth={1.5} />}
          title={t("empty_state.projects_title")}
          subtitle={t("empty_state.projects_subtitle")}
          action={{
            label: t("empty_state.projects_cta"),
            onClick: () => {
              setError(null);
              setDialog({ type: "create" });
            },
          }}
        />
      ) : selected ? (
        <ProjectPage
          project={selected}
          sessionCount={sessionCountByProject.get(selected.id) ?? 0}
          onEdit={() => {
            setError(null);
            setDialog({ type: "edit", project: selected });
          }}
          onDelete={() => {
            setError(null);
            setDialog({ type: "delete", project: selected });
          }}
        />
      ) : (
        <EmptyState
          icon={<FolderGit2 size={28} strokeWidth={1.5} />}
          title={t("projects.select_title", "Select a project")}
          subtitle={t("projects.select_prompt")}
        />
      )}

      {dialog?.type === "create" && (
        <ProjectFormDialog
          mode="create"
          onCancel={() => setDialog(null)}
          onSubmit={async (input) => {
            try {
              const created = await invoke<Project>("create_project", { input });
              setDialog(null);
              await load();
              setSelectedId(created.id);
            } catch (e) {
              setError(String(e));
            }
          }}
          error={error}
          t={t}
        />
      )}

      {dialog?.type === "edit" && (
        <ProjectFormDialog
          mode="edit"
          initial={dialog.project}
          onCancel={() => setDialog(null)}
          onSubmit={async (input) => {
            try {
              const updated: Project = {
                ...dialog.project,
                name: input.name,
                workspace: input.workspace,
                concurrency: input.concurrency ?? dialog.project.concurrency,
                kanbanColumns: input.kanbanColumns ?? dialog.project.kanbanColumns,
                manualReviewAll: input.manualReviewAll ?? dialog.project.manualReviewAll,
              };
              await invoke("update_project", { project: updated });
              setDialog(null);
              await load();
            } catch (e) {
              setError(String(e));
            }
          }}
          error={error}
          t={t}
        />
      )}

      {dialog?.type === "delete" && (
        <ConfirmDialog
          title={t("projects.delete_title")}
          message={t("projects.delete_confirm", { name: dialog.project.name })}
          onCancel={() => setDialog(null)}
          onConfirm={async () => {
            try {
              await invoke("delete_project", { projectId: dialog.project.id });
              setDialog(null);
              if (selectedId === dialog.project.id) setSelectedId(null);
              await load();
            } catch (e) {
              setError(String(e));
            }
          }}
          error={error}
          t={t}
        />
      )}

    </div>
  );
}

function ProjectPage({
  project,
  sessionCount,
  onEdit,
  onDelete,
}: {
  project: Project;
  sessionCount: number;
  onEdit: () => void;
  onDelete: () => void;
}) {
  const { t } = useTranslation();
  const tasks = useTasksStore((s) => s.tasks);
  const refreshTasks = useTasksStore((s) => s.refresh);
  const setSelectedTaskId = useTasksStore((s) => s.setSelectedTaskId);
  const setSelectedProjectId = useProjectsStore((s) => s.setSelectedProjectId);
  const setViewMode = useUIStore((s) => s.setViewMode);
  const [search, setSearch] = useState("");
  const [statusFilter, setStatusFilter] = useState<"all" | TaskStatus>("all");

  useEffect(() => {
    // Fetch the full task list; `projectTasks` below filters to this project
    // client-side. A project-scoped fetch would clobber the sidebar's full list.
    refreshTasks();
  }, [project.id, refreshTasks]);

  const projectTasks = useMemo(
    () => tasks.filter((tk) => tk.projectId === project.id),
    [tasks, project.id],
  );

  const activeCount = projectTasks.filter(
    (tk) => tk.status !== "done" && tk.status !== "abandoned",
  ).length;
  const oneWeekAgo = Date.now() / 1000 - 7 * 24 * 3600;
  const doneThisWeek = projectTasks.filter(
    (tk) => tk.status === "done" && (tk.completedAt ?? 0) >= oneWeekAgo,
  ).length;
  const completedDurations = projectTasks
    .filter((tk) => tk.status === "done" && tk.startedAt && tk.completedAt)
    .map((tk) => (tk.completedAt ?? 0) - (tk.startedAt ?? 0));
  const avgDurationSecs = completedDurations.length
    ? Math.round(
        completedDurations.reduce((acc, n) => acc + n, 0) / completedDurations.length,
      )
    : null;
  const now = Date.now() / 1000;
  const longestRunningSecs = projectTasks
    .filter((tk) => tk.status === "running" && tk.startedAt)
    .reduce((max, tk) => Math.max(max, now - (tk.startedAt ?? 0)), 0);
  const formatDuration = (s: number) => {
    if (s < 60) return `${Math.round(s)}s`;
    if (s < 3600) return `${Math.round(s / 60)}m`;
    if (s < 86400) return `${Math.round(s / 360) / 10}h`;
    return `${Math.round(s / 8640) / 10}d`;
  };

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    return projectTasks
      .filter((tk) => statusFilter === "all" || tk.status === statusFilter)
      .filter(
        (tk) =>
          !q ||
          tk.title.toLowerCase().includes(q) ||
          (tk.description ?? "").toLowerCase().includes(q),
      )
      .sort((a, b) => (b.startedAt ?? b.createdAt) - (a.startedAt ?? a.createdAt));
  }, [projectTasks, statusFilter, search]);

  const openTask = (taskId: string) => {
    setSelectedTaskId(taskId);
    setViewMode("tasks");
  };
  const newTask = () => {
    setSelectedProjectId(project.id);
    setSelectedTaskId(null);
    setViewMode("tasks");
  };

  const planProgress = (tk: (typeof projectTasks)[number]) => {
    const items = Object.values(tk.plan.items ?? {});
    const total = items.length;
    const done = items.filter(
      (it) => typeof it.status === "string" && (it.status === "done" || it.status === "skipped"),
    ).length;
    return { total, done };
  };

  return (
    <div className={styles.project_page}>
      <div className={styles.detail_header}>
        <h3 className={styles.detail_title}>{project.name}</h3>
        <div className={styles.detail_actions}>
          <button type="button" className={styles.new_task_btn} onClick={newTask}>
            + {t("tasks.new_task", "New task")}
          </button>
          <button type="button" onClick={onEdit}>{t("projects.edit")}</button>
          <button type="button" onClick={onDelete} className={styles.danger}>
            {t("projects.delete")}
          </button>
        </div>
      </div>
      <div className={styles.workspace_line} title={project.workspace}>
        {project.workspace}
        <span className={styles.workspace_sep}>·</span>
        {t("projects.concurrency")}: {project.concurrency}
        <span className={styles.workspace_sep}>·</span>
        {t("projects.session_count")}: {sessionCount}
      </div>

      <div className={styles.project_metrics}>
        <div className={styles.metric}>
          <div className={styles.metric_value}>{activeCount}</div>
          <div className={styles.metric_label}>{t("projects.metric_active_tasks")}</div>
        </div>
        <div className={styles.metric}>
          <div className={styles.metric_value}>{doneThisWeek}</div>
          <div className={styles.metric_label}>{t("projects.metric_done_this_week")}</div>
        </div>
        <div className={styles.metric}>
          <div className={styles.metric_value}>{avgDurationSecs != null ? formatDuration(avgDurationSecs) : "—"}</div>
          <div className={styles.metric_label}>{t("projects.metric_avg_duration")}</div>
        </div>
        <div className={styles.metric}>
          <div className={styles.metric_value}>{longestRunningSecs ? formatDuration(longestRunningSecs) : "—"}</div>
          <div className={styles.metric_label}>{t("projects.metric_longest_running")}</div>
        </div>
      </div>

      <div className={styles.task_toolbar}>
        <div className={styles.search_box}>
          <Search size={14} strokeWidth={1.6} />
          <input
            type="text"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder={t("projects.search_tasks", "Search tasks…")}
          />
        </div>
        <select
          className={styles.status_select}
          value={statusFilter}
          onChange={(e) => setStatusFilter(e.target.value as "all" | TaskStatus)}
        >
          {STATUS_FILTERS.map((s) => (
            <option key={s} value={s}>
              {s === "all"
                ? t("projects.filter_all", "All statuses")
                : t(`tasks.status.${s}`, s)}
            </option>
          ))}
        </select>
      </div>

      {projectTasks.length === 0 ? (
        <p className={styles.empty}>{t("empty_state.tasks_title")}</p>
      ) : filtered.length === 0 ? (
        <p className={styles.empty}>{t("projects.no_match", "No tasks match the filter.")}</p>
      ) : (
        <ul className={styles.project_tasks_list}>
          {filtered.map((tk) => {
            const { total, done } = planProgress(tk);
            return (
              <li key={tk.id}>
                <button
                  type="button"
                  className={styles.project_task_row}
                  onClick={() => openTask(tk.id)}
                  title={tk.title}
                >
                  <span className={`${styles.project_task_status} ${styles[`project_task_status_${tk.status}`] ?? ""}`}>
                    {t(`tasks.status.${tk.status}`, tk.status)}
                  </span>
                  <span className={styles.project_task_title}>{tk.title}</span>
                  {total > 0 && (
                    <span className={styles.project_task_progress}>{done}/{total}</span>
                  )}
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}

function ProjectFormDialog({
  mode,
  initial,
  onCancel,
  onSubmit,
  error,
  t,
}: {
  mode: "create" | "edit";
  initial?: Project;
  onCancel: () => void;
  onSubmit: (input: {
    name: string;
    workspace: string;
    concurrency?: number;
    kanbanColumns?: KanbanColumn[];
    manualReviewAll?: boolean;
  }) => void;
  error: string | null;
  t: (k: string, opts?: Record<string, unknown>) => string;
}) {
  const [name, setName] = useState(initial?.name ?? "");
  const [workspace, setWorkspace] = useState(initial?.workspace ?? "");
  const [concurrency, setConcurrency] = useState(initial?.concurrency ?? 1);
  const [manualReviewAll, setManualReviewAll] = useState(
    initial?.manualReviewAll ?? false,
  );
  const [kanbanColumns, setKanbanColumns] = useState<KanbanColumn[]>(() =>
    initial?.kanbanColumns
      ? [...initial.kanbanColumns].sort((a, b) => a.order - b.order)
      : [],
  );

  const renumber = (cols: KanbanColumn[]): KanbanColumn[] =>
    cols.map((c, i) => ({ ...c, order: i }));

  const renameCol = (id: string, newName: string) => {
    setKanbanColumns((prev) =>
      prev.map((c) => (c.id === id ? { ...c, name: newName } : c)),
    );
  };

  const recolorCol = (id: string, newColor: string) => {
    setKanbanColumns((prev) =>
      prev.map((c) => (c.id === id ? { ...c, color: newColor } : c)),
    );
  };

  const deleteCol = (id: string) => {
    setKanbanColumns((prev) => renumber(prev.filter((c) => c.id !== id)));
  };

  const moveCol = (id: string, dir: -1 | 1) => {
    setKanbanColumns((prev) => {
      const idx = prev.findIndex((c) => c.id === id);
      if (idx < 0) return prev;
      const target = idx + dir;
      if (target < 0 || target >= prev.length) return prev;
      const next = [...prev];
      [next[idx], next[target]] = [next[target], next[idx]];
      return renumber(next);
    });
  };

  const addCol = () => {
    const newCol: KanbanColumn = {
      id: `col-${crypto.randomUUID()}`,
      name: t("projects.column_new_default") as string,
      color: "#a78bfa",
      isDefault: false,
      order: kanbanColumns.length,
    };
    setKanbanColumns((prev) => [...prev, newCol]);
  };

  return (
    <div className={styles.modal_overlay} onClick={onCancel}>
      <div className={styles.modal} onClick={(e) => e.stopPropagation()}>
        <h3>{mode === "create" ? t("projects.new") : t("projects.edit")}</h3>
        <label className={styles.field}>
          <span>{t("projects.name")}</span>
          <input
            type="text"
            value={name}
            onChange={(e) => setName(e.target.value)}
            autoFocus
          />
        </label>
        <label className={styles.field}>
          <span>{t("projects.workspace")}</span>
          <div className={styles.field_row}>
            <input
              type="text"
              value={workspace}
              placeholder={t("projects.workspace_placeholder") as string}
              onChange={(e) => setWorkspace(e.target.value)}
            />
            <button
              type="button"
              className={styles.browse_btn}
              onClick={async () => {
                const picked = await openDialog({
                  directory: true,
                  multiple: false,
                  defaultPath: workspace.trim() || undefined,
                });
                if (typeof picked === "string") setWorkspace(picked);
              }}
            >
              {t("projects.browse")}
            </button>
          </div>
        </label>
        <label className={styles.field}>
          <span>{t("projects.concurrency")}</span>
          <input
            type="number"
            min={1}
            max={32}
            value={concurrency}
            onChange={(e) => setConcurrency(Math.max(1, parseInt(e.target.value || "1", 10)))}
          />
        </label>

        <label className={styles.field}>
          <span>{t("projects.manual_review_all")}</span>
          <div className={styles.field_row}>
            <input
              type="checkbox"
              checked={manualReviewAll}
              onChange={(e) => setManualReviewAll(e.target.checked)}
            />
            <span className={styles.hint}>
              {t("projects.manual_review_all_hint")}
            </span>
          </div>
        </label>

        {mode === "edit" && (
          <div className={styles.field}>
            <span>{t("projects.kanban_columns")}</span>
            <div className={styles.column_list}>
              {kanbanColumns.map((col, idx) => (
                <div key={col.id} className={styles.column_row}>
                  <input
                    type="color"
                    className={styles.column_swatch}
                    value={col.color ?? "#a78bfa"}
                    onChange={(e) => recolorCol(col.id, e.target.value)}
                    title={t("projects.column_color") as string}
                  />
                  <input
                    type="text"
                    className={styles.column_name_input}
                    value={col.name}
                    disabled={col.isDefault}
                    onChange={(e) => renameCol(col.id, e.target.value)}
                    title={
                      col.isDefault
                        ? (t("projects.column_default") as string)
                        : (t("projects.column_rename") as string)
                    }
                  />
                  {col.isDefault && (
                    <span className={styles.column_lock} aria-hidden>
                      🔒
                    </span>
                  )}
                  <button
                    type="button"
                    className={styles.column_move_btn}
                    onClick={() => moveCol(col.id, -1)}
                    disabled={idx === 0}
                    title={t("projects.column_move_up") as string}
                    aria-label={t("projects.column_move_up") as string}
                  >
                    ↑
                  </button>
                  <button
                    type="button"
                    className={styles.column_move_btn}
                    onClick={() => moveCol(col.id, 1)}
                    disabled={idx === kanbanColumns.length - 1}
                    title={t("projects.column_move_down") as string}
                    aria-label={t("projects.column_move_down") as string}
                  >
                    ↓
                  </button>
                  <button
                    type="button"
                    className={styles.column_delete_btn}
                    onClick={() => deleteCol(col.id)}
                    disabled={col.isDefault}
                    title={
                      col.isDefault
                        ? (t("projects.column_default") as string)
                        : (t("projects.column_delete") as string)
                    }
                    aria-label={t("projects.column_delete") as string}
                  >
                    ×
                  </button>
                </div>
              ))}
            </div>
            <button type="button" className={styles.column_add_btn} onClick={addCol}>
              + {t("projects.column_add")}
            </button>
          </div>
        )}

        {error && <div className={styles.error}>{error}</div>}
        <div className={styles.modal_actions}>
          <button type="button" onClick={onCancel}>{t("cancel")}</button>
          <button
            type="button"
            onClick={() =>
              onSubmit({
                name: name.trim(),
                workspace: workspace.trim(),
                concurrency,
                manualReviewAll,
                kanbanColumns:
                  mode === "edit"
                    ? kanbanColumns.map((c) => ({ ...c, name: c.name.trim() || c.id }))
                    : undefined,
              })
            }
            disabled={!name.trim() || !workspace.trim()}
          >
            {mode === "create" ? t("projects.create") : t("projects.save")}
          </button>
        </div>
      </div>
    </div>
  );
}

function ConfirmDialog({
  title,
  message,
  onCancel,
  onConfirm,
  error,
  t,
}: {
  title: string;
  message: string;
  onCancel: () => void;
  onConfirm: () => void;
  error: string | null;
  t: (k: string) => string;
}) {
  return (
    <div className={styles.modal_overlay} onClick={onCancel}>
      <div className={styles.modal} onClick={(e) => e.stopPropagation()}>
        <h3>{title}</h3>
        <p>{message}</p>
        {error && <div className={styles.error}>{error}</div>}
        <div className={styles.modal_actions}>
          <button type="button" onClick={onCancel}>{t("cancel")}</button>
          <button type="button" className={styles.danger} onClick={onConfirm}>
            {t("projects.delete")}
          </button>
        </div>
      </div>
    </div>
  );
}
