import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { KanbanSquare } from "lucide-react";
import { useDetailStore, useSessionsStore } from "../store";
import type { SessionInfo, SessionStatus } from "../types";
import { EmptyState } from "./EmptyState";
import styles from "./KanbanView.module.css";

const ACTIVE_STATUSES: ReadonlySet<SessionStatus> = new Set<SessionStatus>([
  "thinking", "executing", "streaming", "processing", "waitingInput", "active", "delegating",
]);

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
  onProjectChanged: _onProjectChanged,
  compact = false,
}: {
  project: Project;
  onProjectChanged: () => void;
  compact?: boolean;
}) {
  const { t } = useTranslation();
  const [sessions, setSessions] = useState<FleetSession[]>([]);
  const [draggingSessionId, setDraggingSessionId] = useState<string | null>(null);
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
    // pull rather than rely on Tauri events. Skip the tick when the page is
    // hidden so a backgrounded window doesn't keep re-rendering the board.
    const tick = () => {
      if (document.visibilityState === "hidden") return;
      refresh();
    };
    const id = window.setInterval(tick, 2000);
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

  const openSessionDetail = useCallback(
    (s: FleetSession) => {
      const liveSessions = useSessionsStore.getState().sessions;
      const live = liveSessions.find((info) => info.id === s.id);
      if (!live) {
        flashToast(t("kanban.toast_session_not_started"));
        return;
      }
      useDetailStore.getState().open(live);
    },
    [t],
  );

  if (!compact && sessions.length === 0) {
    return (
      <EmptyState
        icon={<KanbanSquare size={28} strokeWidth={1.5} />}
        title={t("empty_state.kanban_title")}
        subtitle={t("empty_state.kanban_subtitle")}
      />
    );
  }

  return (
    <div className={`${styles.board} ${compact ? styles.board_compact : ""}`}>
      {sortedColumns.map((col) => (
        <div
          key={col.id}
          className={`${styles.column} ${compact ? styles.column_compact : ""}`}
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
            <span
              className={styles.column_name}
              title={col.isDefault ? t("kanban.default_column_locked") : col.name}
            >
              {col.name}
              {col.isDefault && <span className={styles.lock_icon}>🔒</span>}
            </span>
            <span className={styles.count}>{sessionsByColumn.get(col.id)?.length ?? 0}</span>
          </div>

          <div className={styles.column_body}>
            {(sessionsByColumn.get(col.id) ?? []).map((s) => (
              <KanbanCard
                key={s.id}
                session={s}
                column={col}
                onDragStart={() => setDraggingSessionId(s.id)}
                onDragEnd={() => setDraggingSessionId(null)}
                onOpen={() => openSessionDetail(s)}
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

      {toast && <div className={styles.toast}>{toast}</div>}
    </div>
  );
}

function KanbanCard({
  session,
  column,
  onDragStart,
  onDragEnd,
  onOpen,
  isDragging,
  t,
}: {
  session: FleetSession;
  column: KanbanColumn;
  onDragStart: () => void;
  onDragEnd: () => void;
  onOpen: () => void;
  isDragging: boolean;
  t: (k: string) => string;
}) {
  // Live SessionInfo (real-time transcript scan) keyed by Claude session UUID,
  // which is exactly FleetSession.id. Zustand selector returns the same
  // reference when the underlying SessionInfo hasn't changed, so this only
  // re-renders when this card's session actually updates.
  const liveInfo: SessionInfo | undefined = useSessionsStore((s) =>
    s.sessions.find((info) => info.id === session.id),
  );

  const draggable = !column.isDefault;
  const handleDragStart = (e: React.DragEvent) => {
    e.dataTransfer.setData("text/plain", session.id);
    e.dataTransfer.effectAllowed = "move";
    onDragStart();
  };

  const title =
    liveInfo?.aiTitle?.trim() ||
    session.prompt.slice(0, 60) ||
    session.id.slice(0, 8);
  const speed = liveInfo?.tokenSpeed ?? 0;
  const dotInlineStyle =
    !liveInfo && column.color ? { background: column.color } : undefined;

  const isActive = liveInfo?.status ? ACTIVE_STATUSES.has(liveInfo.status) : false;

  return (
    <div
      className={`${styles.card} ${isDragging ? styles.card_dragging : ""}`}
      data-active={isActive || undefined}
      draggable={draggable}
      onDragStart={draggable ? handleDragStart : undefined}
      onDragEnd={onDragEnd}
      onClick={onOpen}
      title={draggable ? t("kanban.drag_or_click") : t("kanban.click_to_open")}
    >
      <div className={styles.card_header}>
        <span
          className={styles.card_status_dot}
          data-status={liveInfo?.status}
          style={dotInlineStyle}
          aria-hidden
        />
        <span className={styles.card_title}>{title}</span>
        {speed >= 0.5 && (
          <span className={styles.card_speed}>
            {speed.toFixed(1)}
            <span className={styles.card_speed_unit}> {t("tok_s")}</span>
          </span>
        )}
        {session.expedited && <span className={styles.card_chip}>{t("kanban.expedited")}</span>}
      </div>
      {session.note && <div className={styles.card_note}>{session.note}</div>}
    </div>
  );
}
