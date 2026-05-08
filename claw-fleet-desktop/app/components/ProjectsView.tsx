import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { FileBrowserView } from "./FileBrowserView";
import { KanbanView } from "./KanbanView";
import { SessionLauncher, type SessionLauncherProps } from "./SessionLauncher";
import styles from "./ProjectsView.module.css";

interface KanbanColumn {
  id: string;
  name: string;
  color: string | null;
  isDefault: boolean;
  order: number;
}

interface Project {
  id: string;
  name: string;
  workspace: string;
  concurrency: number;
  kanbanColumns: KanbanColumn[];
  createdAt: number;
  updatedAt: number;
}

interface FleetSession {
  id: string;
  projectId: string;
  workspace: string;
  fleetsessionPath: string | null;
  prompt: string;
  contextFiles: string[];
  status: string;
  note: string | null;
  createdAt: number;
  startedAt: number | null;
  completedAt: number | null;
  pid: number | null;
  expedited: boolean;
}

type DialogMode =
  | { type: "create" }
  | { type: "edit"; project: Project }
  | { type: "delete"; project: Project }
  | null;

export function ProjectsView() {
  const { t } = useTranslation();
  const [projects, setProjects] = useState<Project[]>([]);
  const [fleetSessions, setFleetSessions] = useState<FleetSession[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [dialog, setDialog] = useState<DialogMode>(null);
  const [error, setError] = useState<string | null>(null);
  const [launcherInitial, setLauncherInitial] = useState<SessionLauncherProps["initial"]>(null);

  const load = useCallback(async () => {
    try {
      const [projData, sessData] = await Promise.all([
        invoke<Project[]>("list_projects"),
        invoke<FleetSession[]>("list_fleet_sessions").catch(() => []),
      ]);
      setProjects(projData);
      setFleetSessions(sessData);
    } catch {
      setProjects([]);
      setFleetSessions([]);
    } finally {
      setLoaded(true);
    }
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
          <ul className={styles.list_pane} style={{ listStyle: "none", margin: 0 }}>
            {projects.map((p) => (
              <li
                key={p.id}
                className={`${styles.list_item} ${selectedId === p.id ? styles.list_item_active : ""}`}
                onClick={() => setSelectedId(p.id)}
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

          <div className={styles.detail_pane}>
            {selected ? (
              <ProjectDetail
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
  onEdit,
  onDelete,
  onNewSession,
  onProjectChanged,
  t,
}: {
  project: Project;
  sessionCount: number;
  onEdit: () => void;
  onDelete: () => void;
  onNewSession: () => void;
  onProjectChanged: () => void;
  t: (k: string, opts?: Record<string, unknown>) => string;
}) {
  const [tab, setTab] = useState<"kanban" | "files">("kanban");
  return (
    <>
      <div className={styles.detail_header}>
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
      <div style={{ display: "flex", gap: 4, marginBottom: 8 }}>
        <button
          type="button"
          className={tab === "kanban" ? styles.tab_active : styles.tab}
          onClick={() => setTab("kanban")}
        >
          {t("projects.tab_kanban")}
        </button>
        <button
          type="button"
          className={tab === "files" ? styles.tab_active : styles.tab}
          onClick={() => setTab("files")}
        >
          {t("projects.tab_files")}
        </button>
      </div>
      {tab === "kanban" ? (
        <KanbanView project={project} onProjectChanged={onProjectChanged} />
      ) : (
        <FileBrowserView projectId={project.id} workspace={project.workspace} />
      )}
    </>
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
  onSubmit: (input: { name: string; workspace: string; concurrency?: number }) => void;
  error: string | null;
  t: (k: string, opts?: Record<string, unknown>) => string;
}) {
  const [name, setName] = useState(initial?.name ?? "");
  const [workspace, setWorkspace] = useState(initial?.workspace ?? "");
  const [concurrency, setConcurrency] = useState(initial?.concurrency ?? 1);

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
          <input
            type="text"
            value={workspace}
            placeholder="/absolute/path/to/workspace"
            onChange={(e) => setWorkspace(e.target.value)}
          />
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
        {error && <div className={styles.error}>{error}</div>}
        <div className={styles.modal_actions}>
          <button type="button" onClick={onCancel}>{t("cancel")}</button>
          <button
            type="button"
            onClick={() => onSubmit({ name: name.trim(), workspace: workspace.trim(), concurrency })}
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
