import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useProjectsStore, type KanbanColumn, type Project } from "../store";
import { KanbanView } from "./KanbanView";
import { SessionLauncher, type SessionLauncherProps } from "./SessionLauncher";
import styles from "./ProjectsView.module.css";

type DialogMode =
  | { type: "create" }
  | { type: "edit"; project: Project }
  | { type: "delete"; project: Project }
  | null;

export function ProjectsView() {
  const { t } = useTranslation();
  const projects = useProjectsStore((s) => s.projects);
  const fleetSessions = useProjectsStore((s) => s.fleetSessions);
  const loaded = useProjectsStore((s) => s.loaded);
  const selectedId = useProjectsStore((s) => s.selectedProjectId);
  const setSelectedId = useProjectsStore((s) => s.setSelectedProjectId);
  const [dialog, setDialog] = useState<DialogMode>(null);
  const [error, setError] = useState<string | null>(null);
  const [launcherInitial, setLauncherInitial] = useState<SessionLauncherProps["initial"]>(null);
  const [listCollapsed, setListCollapsed] = useState(false);

  const load = useCallback(async () => {
    await useProjectsStore.getState().refresh();
  }, []);

  useEffect(() => {
    load();
  }, [load]);

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
      <div className={styles.header}>
        <h2 className={styles.title}>{t("projects.title")}</h2>
        <button
          type="button"
          className={styles.action_btn}
          onClick={() => {
            setError(null);
            setDialog({ type: "create" });
          }}
        >
          + {t("projects.new")}
        </button>
      </div>

      {!loaded ? (
        <p className={styles.empty}>{t("scanning")}</p>
      ) : projects.length === 0 ? (
        <p className={styles.empty}>{t("projects.empty")}</p>
      ) : (
        <div className={styles.body}>
          {!listCollapsed && (
            <ul className={styles.list_pane} style={{ listStyle: "none", margin: 0 }}>
              {projects.map((p) => (
                <li
                  key={p.id}
                  className={`${styles.list_item} ${selectedId === p.id ? styles.list_item_active : ""}`}
                  onClick={() => {
                    setSelectedId(p.id);
                    setListCollapsed(true);
                  }}
                >
                  <div className={styles.list_item_title}>{p.name}</div>
                  <div className={styles.list_item_sub}>
                    <span style={{ flex: 1, overflow: "hidden", textOverflow: "ellipsis" }}>
                      {p.workspace}
                    </span>
                    <span className={styles.chip}>
                      {sessionCountByProject.get(p.id) ?? 0}
                    </span>
                  </div>
                </li>
              ))}
            </ul>
          )}

          <div className={styles.detail_pane}>
            {selected ? (
              <ProjectDetail
                project={selected}
                sessionCount={sessionCountByProject.get(selected.id) ?? 0}
                listCollapsed={listCollapsed}
                onToggleList={() => setListCollapsed((v) => !v)}
                onEdit={() => {
                  setError(null);
                  setDialog({ type: "edit", project: selected });
                }}
                onDelete={() => {
                  setError(null);
                  setDialog({ type: "delete", project: selected });
                }}
                onNewSession={() => {
                  setLauncherInitial({ projectId: selected.id });
                }}
                onProjectChanged={load}
                t={t}
              />
            ) : (
              <p className={styles.empty}>{t("projects.select_prompt")}</p>
            )}
          </div>
        </div>
      )}

      {dialog?.type === "create" && (
        <ProjectFormDialog
          mode="create"
          onCancel={() => setDialog(null)}
          onSubmit={async (input) => {
            try {
              await invoke<Project>("create_project", { input });
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

      <SessionLauncher
        initial={launcherInitial}
        onClose={() => setLauncherInitial(null)}
        onSubmitted={() => load()}
      />
    </div>
  );
}

function ProjectDetail({
  project,
  sessionCount,
  listCollapsed,
  onToggleList,
  onEdit,
  onDelete,
  onNewSession,
  onProjectChanged,
  t,
}: {
  project: Project;
  sessionCount: number;
  listCollapsed: boolean;
  onToggleList: () => void;
  onEdit: () => void;
  onDelete: () => void;
  onNewSession: () => void;
  onProjectChanged: () => void;
  t: (k: string, opts?: Record<string, unknown>) => string;
}) {
  return (
    <div className={styles.main_pane}>
      <div className={styles.detail_main}>
        <div className={styles.detail_header}>
          <button
            type="button"
            className={styles.list_toggle}
            onClick={onToggleList}
            title={listCollapsed ? t("projects.show_list") : t("projects.hide_list")}
            aria-label={listCollapsed ? t("projects.show_list") : t("projects.hide_list")}
          >
            {listCollapsed ? "›" : "‹"}
          </button>
          <h3 className={styles.detail_title}>{project.name}</h3>
          <div className={styles.detail_actions}>
            <button type="button" onClick={onNewSession}>+ {t("launcher.new_session")}</button>
            <button type="button" onClick={onEdit}>{t("projects.edit")}</button>
            <button type="button" onClick={onDelete} className={styles.danger}>
              {t("projects.delete")}
            </button>
          </div>
        </div>
        <dl className={styles.kv} style={{ marginBottom: 8 }}>
          <dt>{t("projects.workspace")}</dt>
          <dd>{project.workspace}</dd>
          <dt>{t("projects.concurrency")}</dt>
          <dd>{project.concurrency}</dd>
          <dt>{t("projects.session_count")}</dt>
          <dd>{sessionCount}</dd>
        </dl>
      </div>
      <div className={styles.kanban_pane}>
        <KanbanView project={project} onProjectChanged={onProjectChanged} compact />
      </div>
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
  }) => void;
  error: string | null;
  t: (k: string, opts?: Record<string, unknown>) => string;
}) {
  const [name, setName] = useState(initial?.name ?? "");
  const [workspace, setWorkspace] = useState(initial?.workspace ?? "");
  const [concurrency, setConcurrency] = useState(initial?.concurrency ?? 1);
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
              placeholder="/absolute/path/to/workspace"
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
