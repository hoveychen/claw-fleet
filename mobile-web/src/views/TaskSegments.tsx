// The task detail page's segment switch: 对话 / 子任务 / 步骤 / 资料.
//
// 对话 is the transcript the page already shows; the other three are panes
// laid over it. 步骤 reuses TaskPlansTab as it is. 子任务 and 资料 live here.
// The developer panes (token, notes, workflow, handoff, …) stay behind the ⋮
// sheet — they describe the session, not the task.

import { useMemo } from "react";
import { ChevronRight, FolderOpen, GitFork, WifiOff } from "lucide-react";
import { t } from "../i18n";
import type { FleetTransport } from "../transport";
import type { SessionInfo, WikiDoc } from "../types";
import { subtaskRowsOf, taskSessionIds, taskTitle } from "../taskRows";
import { taskLibrary } from "../library";
import { LibraryList, useLibrary } from "./LibraryList";
import { isWakingSticky, statusTone, subtaskStateLabel, timeAgo } from "./TasksView";
import { EmptyState } from "./EmptyState";
import { SkeletonList } from "./loading";
import styles from "./TaskSegments.module.css";

export type TaskSegment = "chat" | "subtasks" | "steps" | "library";

const SEGMENT_LABEL: Record<TaskSegment, string> = {
  chat: "对话",
  subtasks: "子任务",
  steps: "步骤",
  library: "资料",
};

/** Which segments a session's page offers. A sub-task's own page has no
 *  子任务 segment — sub-tasks hang off the main conversation only. */
export function taskSegments(session: SessionInfo): TaskSegment[] {
  if (session.isSubagent) return [];
  return session.parentTaskId
    ? ["chat", "steps", "library"]
    : ["chat", "subtasks", "steps", "library"];
}

export function SegmentBar({
  segments,
  value,
  onChange,
  subtaskCount,
}: {
  segments: TaskSegment[];
  value: TaskSegment;
  onChange: (s: TaskSegment) => void;
  subtaskCount?: number;
}) {
  return (
    <div className={styles.bar} role="tablist">
      {segments.map((s) => (
        <button
          key={s}
          type="button"
          role="tab"
          aria-selected={value === s}
          className={styles.seg}
          data-active={value === s}
          onClick={() => onChange(s)}
        >
          {t(SEGMENT_LABEL[s])}
          {s === "subtasks" && !!subtaskCount && (
            <span className={styles.segCount}>{subtaskCount}</span>
          )}
        </button>
      ))}
    </div>
  );
}

export function useSubtasks(
  session: SessionInfo,
  sessions: SessionInfo[],
  pendingSessionIds?: ReadonlySet<string>,
) {
  return useMemo(
    () =>
      session.taskId && !session.parentTaskId
        ? subtaskRowsOf(sessions, session.taskId, {
            isPending: (s) => !!pendingSessionIds?.has(s.id),
            isQuiet: (s) => statusTone(s) === "quiet",
            isWaking: (s, pending) => isWakingSticky(s, pending),
          })
        : [],
    [session.taskId, session.parentTaskId, sessions, pendingSessionIds],
  );
}

export function SubtasksPane({
  rows,
  onOpenSessionId,
}: {
  rows: ReturnType<typeof useSubtasks>;
  onOpenSessionId: (id: string) => void;
}) {
  if (rows.length === 0) {
    return (
      <EmptyState
        compact
        icon={GitFork}
        title={t("还没有子任务")}
        description={t("这个任务派出的子任务会列在这里。")}
      />
    );
  }
  return (
    <div className={styles.card}>
      {rows.map((r) => (
        <button
          key={r.key}
          type="button"
          className={styles.row}
          onClick={() => onOpenSessionId(r.tip.id)}
        >
          <span className={styles.dot} data-state={r.state} />
          <span className={styles.rowBody}>
            <span className={styles.rowTitle}>{taskTitle(r.tip, t("未命名任务"))}</span>
            <span className={styles.rowMeta}>
              {r.pending ? t("等你回答") : subtaskStateLabel(r.state)}
              {r.hops.length > 1 && ` · ${t("{0} 段", r.hops.length)}`}
            </span>
          </span>
          <span className={styles.rowTime}>{timeAgo(r.tip.lastActivityMs)}</span>
          <ChevronRight size={16} className={styles.chevron} />
        </button>
      ))}
    </div>
  );
}

export function TaskLibraryPane({
  session,
  sessions,
  client,
  onOpenDoc,
}: {
  session: SessionInfo;
  sessions: SessionInfo[];
  client: FleetTransport | null;
  onOpenDoc: (doc: WikiDoc) => void;
}) {
  const lib = useLibrary(client);
  const ids = useMemo(() => taskSessionIds(sessions, session), [sessions, session]);
  const scoped = useMemo(
    () => (lib.items ? taskLibrary(lib.items, ids, session.workspacePath) : null),
    [lib.items, ids, session.workspacePath],
  );

  if (!client) return <EmptyState compact icon={WifiOff} title={t("桌面端离线")} />;
  if (!scoped) return <SkeletonList rows={4} avatar />;
  return (
    <div className={styles.libraryPane}>
      {lib.error && <div className={styles.error}>{t("资料加载失败：{0}", lib.error)}</div>}
      <div className={styles.groupHead}>{t("本任务产出")}</div>
      {scoped.outputs.length > 0 ? (
        <LibraryList items={scoped.outputs} client={client} onOpenDoc={onOpenDoc} hideProject />
      ) : (
        <div className={styles.groupEmpty}>{t("这个任务还没有存进产出库的文件。")}</div>
      )}
      <div className={styles.groupHead}>
        <FolderOpen size={13} />
        {t("{0} 的文档", session.workspaceName || t("本项目"))}
      </div>
      {scoped.docs.length > 0 ? (
        <LibraryList items={scoped.docs} client={client} onOpenDoc={onOpenDoc} hideProject />
      ) : (
        <div className={styles.groupEmpty}>{t("这个项目还没有归档的文档。")}</div>
      )}
    </div>
  );
}
