import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import styles from "./KanbanView.module.css";

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

export function KanbanView({
  project,
  onProjectChanged,
}: {
  project: Project;
  onProjectChanged: () => void;
}) {
  const { t } = useTranslation();
  const [sessions, setSessions] = useState<FleetSession[]>([]);
  const [editingColumnId, setEditingColumnId] = useState<string | null>(null);
  const [draggingSessionId, setDraggingSessionId] = useState<string | null>(null);
  const [confirmDeleteCol, setConfirmDeleteCol] = useState<KanbanColumn | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const refreshTimer = useRef<number | null>(null);

  const refresh = useCallback(async () => {
    try {
      const all = await invoke<FleetSession[]>("list_fleet_sessions");
      setSessions(all.filter((s) => s.projectId === project.id));
    } catch {
      setSessions([]);
    }
  }, [project.id]);

  useEffect(() => {
    refresh();
    // Poll every 2s while the kanban is open. P3 supervisor mutates
    // fleet-sessions.json from a separate thread / process, so we need to
    // pull rather than rely on Tauri events.
    const id = window.setInterval(refresh, 2000);
    refreshTimer.current = id;
    return () => {
      if (refreshTimer.current != null) window.clearInterval(refreshTimer.current);
    };
  }, [refresh]);

  const sortedColumns = useMemo(
    () => [...project.kanbanColumns].sort((a, b) => a.order - b.order),
    [project.kanbanColumns],
  );

  const sessionsByColumn = useMemo(() => {
    const map = new Map<string, FleetSession[]>();
    for (const c of sortedColumns) map.set(c.id, []);
    for (const s of sessions) {
      const col = map.get(s.status);
      if (col) col.push(s);
      else map.get("queued")?.push(s); // fallback bucket
    }
    return map;
  }, [sessions, sortedColumns]);

  const flashToast = (msg: string) => {
    setToast(msg);
    window.setTimeout(() => setToast(null), 2400);
  };

  const handleDrop = async (sessionId: string, targetColumnId: string) => {
    const target = project.kanbanColumns.find((c) => c.id === targetColumnId);
    if (!target) return;
    if (target.isDefault) {
      flashToast(t("kanban.toast_default_locked"));
      return;
    }
    try {
      await invoke("set_fleet_session_status", {
        sessionId,
        status: targetColumnId,
        note: null,
      });
      await refresh();
    } catch (e) {
      flashToast(String(e));
    }
  };

  const addColumn = async () => {
    const name = window.prompt(t("kanban.new_column_prompt"));
    if (!name || !name.trim()) return;
    const id = `col-${crypto.randomUUID()}`;
    const newCol: KanbanColumn = {
      id,
      name: name.trim(),
      color: "#a78bfa",
      isDefault: false,
      order: (sortedColumns[sortedColumns.length - 1]?.order ?? 0) + 1,
    };
    try {
      await invoke("update_project", {
        project: { ...project, kanbanColumns: [...project.kanbanColumns, newCol] },
      });
      onProjectChanged();
    } catch (e) {
      flashToast(String(e));
    }
  };

  const renameColumn = async (col: KanbanColumn, newName: string) => {
    if (col.isDefault) return;
    if (!newName.trim()) return;
    try {
      await invoke("update_project", {
        project: {
          ...project,
          kanbanColumns: project.kanbanColumns.map((c) =>
            c.id === col.id ? { ...c, name: newName.trim() } : c,
          ),
        },
      });
      onProjectChanged();
    } catch (e) {
      flashToast(String(e));
    }
  };

  const deleteColumn = async (col: KanbanColumn) => {
    if (col.isDefault) return;
    const remaining = sessions.filter((s) => s.status === col.id);
    if (remaining.length > 0) {
      flashToast(t("kanban.toast_must_clear", { count: remaining.length }));
      return;
    }
    try {
      await invoke("update_project", {
        project: {
          ...project,
          kanbanColumns: project.kanbanColumns.filter((c) => c.id !== col.id),
        },
      });
      onProjectChanged();
      setConfirmDeleteCol(null);
    } catch (e) {
      flashToast(String(e));
    }
  };

  return (
    <div className={styles.board}>
      {sortedColumns.map((col) => (
        <div
          key={col.id}
          className={styles.column}
          onDragOver={(e) => {
            e.preventDefault();
          }}
          onDrop={(e) => {
            e.preventDefault();
            const id = e.dataTransfer.getData("text/plain");
            if (id) handleDrop(id, col.id);
            setDraggingSessionId(null);
          }}
        >
          <div
            className={styles.column_header}
            style={col.color ? { borderTopColor: col.color } : undefined}
          >
            {editingColumnId === col.id ? (
              <input
                type="text"
                className={styles.column_name_input}
                defaultValue={col.name}
                autoFocus
                onBlur={(e) => {
                  if (e.target.value !== col.name) renameColumn(col, e.target.value);
                  setEditingColumnId(null);
                }}
                onKeyDown={(e) => {
                  if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                  if (e.key === "Escape") setEditingColumnId(null);
                }}
              />
            ) : (
              <span
                className={styles.column_name}
                onDoubleClick={() => !col.isDefault && setEditingColumnId(col.id)}
                title={col.isDefault ? t("kanban.default_column_locked") : t("kanban.double_click_to_rename")}
              >
                {col.name}
                {col.isDefault && <span className={styles.lock_icon}>🔒</span>}
              </span>
            )}
            <span className={styles.count}>{sessionsByColumn.get(col.id)?.length ?? 0}</span>
            {!col.isDefault && (
              <button
                type="button"
                className={styles.column_delete}
                onClick={() => setConfirmDeleteCol(col)}
                title={t("kanban.delete_column")}
              >
                ×
              </button>
            )}
          </div>

          <div className={styles.column_body}>
            {(sessionsByColumn.get(col.id) ?? []).map((s) => (
              <KanbanCard
                key={s.id}
                session={s}
                column={col}
                onDragStart={() => setDraggingSessionId(s.id)}
                onDragEnd={() => setDraggingSessionId(null)}
                isDragging={draggingSessionId === s.id}
                t={t}
              />
            ))}
            {sessionsByColumn.get(col.id)?.length === 0 && (
              <div className={styles.column_empty}>{t("kanban.column_empty")}</div>
            )}
          </div>
        </div>
      ))}

      <button type="button" className={styles.add_column_btn} onClick={addColumn}>
        + {t("kanban.add_column")}
      </button>

      {confirmDeleteCol && (
        <div className={styles.toast_overlay} onClick={() => setConfirmDeleteCol(null)}>
          <div className={styles.toast_modal} onClick={(e) => e.stopPropagation()}>
            <p>{t("kanban.confirm_delete_column", { name: confirmDeleteCol.name })}</p>
            <div className={styles.toast_actions}>
              <button type="button" onClick={() => setConfirmDeleteCol(null)}>
                {t("cancel")}
              </button>
              <button
                type="button"
                className={styles.danger}
                onClick={() => deleteColumn(confirmDeleteCol)}
              >
                {t("kanban.delete_column")}
              </button>
            </div>
          </div>
        </div>
      )}

      {toast && <div className={styles.toast}>{toast}</div>}
    </div>
  );
}

function KanbanCard({
  session,
  column,
  onDragStart,
  onDragEnd,
  isDragging,
  t,
}: {
  session: FleetSession;
  column: KanbanColumn;
  onDragStart: () => void;
  onDragEnd: () => void;
  isDragging: boolean;
  t: (k: string) => string;
}) {
  const draggable = !column.isDefault;
  const handleDragStart = (e: React.DragEvent) => {
    e.dataTransfer.setData("text/plain", session.id);
    e.dataTransfer.effectAllowed = "move";
    onDragStart();
  };

  return (
    <div
      className={`${styles.card} ${isDragging ? styles.card_dragging : ""}`}
      draggable={draggable}
      onDragStart={draggable ? handleDragStart : undefined}
      onDragEnd={onDragEnd}
      title={draggable ? t("kanban.drag_to_move") : t("kanban.system_managed")}
    >
      <div className={styles.card_header}>
        <span
          className={styles.card_status_dot}
          style={column.color ? { background: column.color } : undefined}
          aria-hidden
        />
        <span className={styles.card_title}>{session.prompt.slice(0, 60) || session.id.slice(0, 8)}</span>
        {session.expedited && <span className={styles.card_chip}>{t("kanban.expedited")}</span>}
      </div>
      {session.note && <div className={styles.card_note}>{session.note}</div>}
    </div>
  );
}
