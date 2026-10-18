import { memo, type MouseEvent } from "react";
import { useTranslation } from "react-i18next";
import { ChevronDown, ChevronRight } from "lucide-react";
import type { TFunction } from "i18next";
import type { TaskState } from "../../../shared-ts/taskState";
import type { SessionInfo } from "../types";
import { timeAgo } from "./timeAgo";
import {
  rowDetail,
  type HomeRow,
  type HomeSection,
  type RowDetail,
  type TaskRow,
} from "./homeRows";
import styles from "./TaskList.module.css";

/** The dot a state wears, in the row and its section heading. Tokens only:
 *  red = needs you, blue = running, amber = waiting, green = to accept. */
export const STATE_DOT: Record<TaskState, string> = {
  needsYou: "var(--color-error)",
  running: "var(--badge-active-fg)",
  waiting: "var(--color-warning)",
  review: "var(--color-success)",
  done: "var(--color-text-dim)",
};

/** "42分" / "1时05分" / "3天": the span a row has been at it. */
export function formatSpan(ms: number, t: TFunction): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  if (s < 60) return t("home.span_s", { n: s });
  const m = Math.floor(s / 60);
  if (m < 60) return t("home.span_m", { n: m });
  const h = Math.floor(m / 60);
  if (h < 24) return t("home.span_hm", { h, m: String(m % 60).padStart(2, "0") });
  return t("home.span_d", { n: Math.floor(h / 24) });
}

function detailText(d: RowDetail, row: HomeRow, t: TFunction, now: number): string {
  switch (d.kind) {
    case "needsYou":
      return row.kind === "card" ? t("home.detail_card_only") : t("home.detail_needs_you");
    case "subtaskNeedsYou":
      return t("home.detail_subtask_needs_you", { title: d.subtaskTitle });
    case "error":
      return d.status === "stuck" && d.tool
        ? t("home.detail_stuck_tool", { tool: d.tool })
        : t("home.detail_error", { status: t(`status.${d.status}`, d.status) });
    case "outOfCredits":
      return t("home.detail_out_of_credits");
    case "step": {
      const parts: string[] = [];
      if (d.total) parts.push(t("home.detail_step", { done: d.done, total: d.total }));
      parts.push(d.text ?? t("home.detail_working"));
      if (d.subtasks) parts.push(t("home.detail_subtasks", d.subtasks));
      return parts.join(" · ");
    }
    case "wait":
      return `${d.reason} · ${t("home.detail_waited", { span: formatSpan(now - d.sinceMs, t) })}`;
    case "waitSubtasks":
      return `${t("home.detail_wait_subtasks", { active: d.active, total: d.total })} · ${t(
        "home.detail_waited",
        { span: formatSpan(now - d.sinceMs, t) },
      )}`;
    case "review":
      return d.total
        ? `${t("home.detail_step", { done: d.done, total: d.total })} · ${t("home.detail_review")}`
        : t("home.detail_review");
    case "none":
      return "";
  }
}

/** Right-hand time: how long a live task has been at it; when a settled one
 *  last moved. A parked task says how long it has waited on its second line
 *  instead. */
function rowTime(row: HomeRow, t: TFunction, now: number): string {
  switch (row.state) {
    case "needsYou":
    case "running":
      return formatSpan(now - row.startedAtMs, t);
    case "waiting":
      return "";
    default:
      return timeAgo(row.lastActivityMs, t);
  }
}

interface RowProps {
  row: HomeRow;
  active: boolean;
  /** Re-render tick for the relative times. */
  nowTick: number;
  accepting: boolean;
  onOpen: (row: HomeRow) => void;
  onContextMenu: (e: MouseEvent, row: HomeRow) => void;
  onScopeProject: (path: string) => void;
  onAccept: (rows: TaskRow[]) => void;
}

const TaskListRow = memo(function TaskListRow({
  row,
  active,
  accepting,
  onOpen,
  onContextMenu,
  onScopeProject,
  onAccept,
}: RowProps) {
  const { t } = useTranslation();
  const now = Date.now();
  const detail = detailText(rowDetail(row, t("history.untitled")), row, t, now);
  const time = rowTime(row, t, now);
  return (
    <div
      role="button"
      tabIndex={0}
      className={`${styles.row} ${active ? styles.row_active : ""}`}
      data-testid="task-row"
      data-state={row.state}
      onClick={() => onOpen(row)}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onOpen(row);
        }
      }}
      onContextMenu={(e) => onContextMenu(e, row)}
    >
      <div className={styles.line1}>
        <span className={styles.dot} style={{ background: STATE_DOT[row.state] }} />
        <span className={styles.title} title={row.title}>
          {row.title}
        </span>
        {row.projectPath ? (
          <button
            type="button"
            className={styles.chip}
            title={t("home.project_filter_tip")}
            onClick={(e) => {
              e.stopPropagation();
              onScopeProject(row.projectPath!);
            }}
          >
            {row.projectName}
          </button>
        ) : (
          <span className={styles.chip}>{row.projectName}</span>
        )}
        <span className={styles.time}>{time}</span>
      </div>
      {(detail || row.state === "review") && (
        <div className={styles.line2}>
          <span className={styles.detail} title={detail}>
            {detail}
          </span>
          {row.kind === "task" && row.state === "review" && (
            <button
              type="button"
              className={styles.accept}
              title={t("home.accept_tip")}
              disabled={accepting}
              onClick={(e) => {
                e.stopPropagation();
                onAccept([row]);
              }}
            >
              {t("home.accept")}
            </button>
          )}
        </div>
      )}
    </div>
  );
});

export interface TaskListProps {
  sections: HomeSection[];
  activeKey: string | null;
  collapsed: TaskState[];
  onToggleSection: (state: TaskState) => void;
  nowTick: number;
  acceptingKeys: Set<string>;
  onOpen: (row: HomeRow) => void;
  onContextMenu: (e: MouseEvent, row: HomeRow) => void;
  onScopeProject: (path: string) => void;
  onAccept: (rows: TaskRow[]) => void;
}

/** The home's task column: the five state sections, each a list of two-line
 *  task rows. */
export function TaskList({
  sections,
  activeKey,
  collapsed,
  onToggleSection,
  nowTick,
  acceptingKeys,
  onOpen,
  onContextMenu,
  onScopeProject,
  onAccept,
}: TaskListProps) {
  const { t } = useTranslation();
  return (
    <>
      {sections.map((section) => {
        const folded = collapsed.includes(section.state);
        const reviewable = section.rows.filter(
          (r): r is TaskRow => r.kind === "task" && !acceptingKeys.has(r.key),
        );
        return (
          <section key={section.state} className={styles.section} data-section={section.state}>
            <div className={styles.section_head}>
              <button
                type="button"
                className={styles.section_toggle}
                aria-expanded={!folded}
                onClick={() => onToggleSection(section.state)}
              >
                {folded ? (
                  <ChevronRight size={14} strokeWidth={2} />
                ) : (
                  <ChevronDown size={14} strokeWidth={2} />
                )}
                <span className={styles.section_label}>{t(`home.section_${section.state}`)}</span>
                <span className={styles.section_count}>{section.rows.length}</span>
              </button>
              {section.state === "review" && reviewable.length > 0 && (
                <button
                  type="button"
                  className={styles.accept_all}
                  onClick={() => onAccept(reviewable)}
                >
                  {t("home.accept_all")}
                </button>
              )}
            </div>
            {!folded &&
              section.rows.map((row) => (
                <TaskListRow
                  key={row.key}
                  row={row}
                  active={row.key === activeKey}
                  nowTick={nowTick}
                  accepting={acceptingKeys.has(row.key)}
                  onOpen={onOpen}
                  onContextMenu={onContextMenu}
                  onScopeProject={onScopeProject}
                  onAccept={onAccept}
                />
              ))}
          </section>
        );
      })}
    </>
  );
}

/** The session a row's context menu acts on. */
export function rowMenuSession(row: HomeRow): SessionInfo | null {
  return row.kind === "card" ? row.session : row.tip;
}
