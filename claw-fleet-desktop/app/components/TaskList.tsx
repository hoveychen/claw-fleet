import { memo, useState, type MouseEvent, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Bot, Check, ChevronDown, ChevronRight, Folder, X } from "lucide-react";
import type { TFunction } from "i18next";
import type { TaskState } from "../../../shared-ts/taskState";
import { speedTier } from "../../../shared-ts/taskSpeed";
import type { SessionInfo } from "../types";
import { timeAgo } from "./timeAgo";
import {
  rowAgentSource,
  rowDetail,
  rowPreview,
  rowActiveSubtasks,
  rowRunningSubagents,
  rowTokenSpeed,
  type HomeRow,
  type HomeSection,
  type ProjectGroup,
  type RowDetail,
  type TaskRow,
} from "./homeRows";
import { AgentSourceIcon } from "./SessionCard";
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

/** `hasPreview`: the row shows the last message after this text, which says
 *  more than the generic "working" filler, so the filler is left out. */
function detailText(
  d: RowDetail,
  row: HomeRow,
  t: TFunction,
  now: number,
  hasPreview = false,
): string {
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
      if (d.text) parts.push(d.text);
      else if (!hasPreview) parts.push(t("home.detail_working"));
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

/** The state dot. Its colour is the state; it pulses at the task's speed tier
 *  (`shared-ts/taskSpeed.ts`), so a row generating hard reads apart from one
 *  waiting on a tool without opening it. */
function StateDot({ row }: { row: HomeRow }) {
  const speed = rowTokenSpeed(row);
  return (
    <span
      className={styles.dot}
      style={{ color: STATE_DOT[row.state] }}
      data-speed={speedTier(speed)}
      data-testid="task-dot"
      title={speed >= 1 ? `${Math.round(speed)} tok/s` : undefined}
    />
  );
}

/** The row's leading mark. A task row's is a toggle — the state dot that, on
 *  hover, offers to mark the task done; a closed task shows how it was closed
 *  (a tick for completed, a cross for abandoned) and clicking reopens it. A
 *  card row has no task to close, so its dot is just a dot. */
function RowMark({
  row,
  onSetDone,
}: {
  row: HomeRow;
  onSetDone: (row: TaskRow, done: boolean) => void;
}) {
  const { t } = useTranslation();
  if (row.kind !== "task") {
    return <StateDot row={row} />;
  }
  const closed = row.state === "done";
  const tip = !closed
    ? t("home.mark_done_tip")
    : row.outcome === "abandoned"
      ? t("home.mark_abandoned_tip")
      : t("home.mark_completed_tip");
  return (
    <button
      type="button"
      className={`${styles.mark} ${closed ? styles.mark_closed : ""}`}
      data-testid="task-mark"
      data-outcome={closed ? (row.outcome ?? "completed") : undefined}
      title={tip}
      aria-label={tip}
      onClick={(e) => {
        e.stopPropagation();
        onSetDone(row, !closed);
      }}
    >
      {closed ? (
        row.outcome === "abandoned" ? (
          <X size={12} strokeWidth={2.5} />
        ) : (
          <Check size={12} strokeWidth={2.5} />
        )
      ) : (
        <>
          <StateDot row={row} />
          <Check className={styles.mark_hover} size={12} strokeWidth={2.5} />
        </>
      )}
    </button>
  );
}

interface RowProps {
  row: HomeRow;
  active: boolean;
  /** Re-render tick for the relative times. */
  nowTick: number;
  accepting: boolean;
  /** Rows come from more than one agent: lead the title with its glyph. */
  showSource: boolean;
  /** Show the project chip — off when the row already sits under its
   *  project's heading. */
  showProject: boolean;
  onOpen: (row: HomeRow) => void;
  onContextMenu: (e: MouseEvent, row: HomeRow) => void;
  onScopeProject: (path: string) => void;
  onAccept: (rows: TaskRow[]) => void;
  onSetDone: (row: TaskRow, done: boolean) => void;
}

const TaskListRow = memo(function TaskListRow({
  row,
  active,
  accepting,
  showSource,
  showProject,
  onOpen,
  onContextMenu,
  onScopeProject,
  onAccept,
  onSetDone,
}: RowProps) {
  const { t } = useTranslation();
  const now = Date.now();
  const abandoned = row.kind === "task" && row.state === "done" && row.outcome === "abandoned";
  const source = showSource ? rowAgentSource(row) : null;
  const subagents = rowRunningSubagents(row);
  const subtasks = rowActiveSubtasks(row);
  const helpersTip = [
    subtasks > 0 ? t("home.running_subtasks", { count: subtasks }) : null,
    subagents > 0 ? t("home.running_subagents", { count: subagents }) : null,
  ]
    .filter(Boolean)
    .join(" · ");
  const preview = rowPreview(row);
  const detail =
    detailText(rowDetail(row, t("history.untitled")), row, t, now, preview != null) ||
    (abandoned ? t("home.detail_abandoned") : "");
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
        <RowMark row={row} onSetDone={onSetDone} />
        {source && (
          <span className={styles.source} title={source} data-testid="task-row-source">
            <AgentSourceIcon source={source} />
          </span>
        )}
        <span className={styles.title} title={row.title}>
          {row.title}
        </span>
        {subagents + subtasks > 0 && (
          <span
            className={styles.subagents}
            title={helpersTip}
            data-testid="task-row-subagents"
          >
            <Bot size={11} strokeWidth={1.8} />
            {subagents + subtasks}
          </span>
        )}
        {!showProject ? null : row.projectPath ? (
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
      {(detail || preview || row.state === "review") && (
        <div className={styles.line2}>
          {detail && (
            <span className={styles.detail} title={detail}>
              {detail}
            </span>
          )}
          {preview && (
            <span className={styles.preview} title={preview} data-testid="task-row-preview">
              {preview}
            </span>
          )}
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
  /** The column cut by state (default) or by project. */
  layout: { by: "state"; sections: HomeSection[] } | { by: "project"; groups: ProjectGroup[] };
  activeKey: string | null;
  collapsed: TaskState[];
  onToggleSection: (state: TaskState) => void;
  /** Project groups folded shut (by-project layout). */
  collapsedProjects: string[];
  onToggleProject: (key: string) => void;
  /** Rows come from more than one agent (see `mixedAgentSources`). */
  showSource: boolean;
  nowTick: number;
  acceptingKeys: Set<string>;
  onOpen: (row: HomeRow) => void;
  onContextMenu: (e: MouseEvent, row: HomeRow) => void;
  onScopeProject: (path: string) => void;
  onAccept: (rows: TaskRow[]) => void;
  /** Mark a task done (`true`, the accept path) or reopen it (`false`). */
  onSetDone: (row: TaskRow, done: boolean) => void;
}

/** The home's task column: two-line task rows, cut into the five state
 *  sections or into one group per project. */
export function TaskList(props: TaskListProps) {
  const { t } = useTranslation();
  const {
    layout,
    activeKey,
    collapsed,
    onToggleSection,
    nowTick,
    acceptingKeys,
    onAccept,
  } = props;
  const renderRow = (row: HomeRow, showProject: boolean) => (
    <TaskListRow
      key={row.key}
      row={row}
      active={row.key === activeKey}
      nowTick={nowTick}
      accepting={acceptingKeys.has(row.key)}
      showSource={props.showSource}
      showProject={showProject}
      onOpen={props.onOpen}
      onContextMenu={props.onContextMenu}
      onScopeProject={props.onScopeProject}
      onAccept={onAccept}
      onSetDone={props.onSetDone}
    />
  );
  if (layout.by === "project") {
    return (
      <>
        {layout.groups.map((g) => (
          <ProjectGroupSection
            key={g.key}
            group={g}
            folded={props.collapsedProjects.includes(g.key)}
            onToggle={props.onToggleProject}
            onScopeProject={props.onScopeProject}
            renderRow={(r) => renderRow(r, false)}
          />
        ))}
      </>
    );
  }
  return (
    <>
      {layout.sections.map((section) => {
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
            {!folded && section.rows.map((row) => renderRow(row, true))}
          </section>
        );
      })}
    </>
  );
}

/** One project in the by-project layout: a heading that says how many of its
 *  tasks need the boss, the open rows (state dot on each, "needs you" first),
 *  and the closed ones folded behind a count — a long-lived project has
 *  hundreds, and they would bury the live work. */
function ProjectGroupSection({
  group,
  folded,
  onToggle,
  onScopeProject,
  renderRow,
}: {
  group: ProjectGroup;
  folded: boolean;
  onToggle: (key: string) => void;
  onScopeProject: (path: string) => void;
  renderRow: (row: HomeRow) => ReactNode;
}) {
  const { t } = useTranslation();
  const [showDone, setShowDone] = useState(false);
  const total = group.rows.length + group.doneRows.length;
  return (
    <section className={styles.section} data-project={group.key} data-testid="project-group">
      <div className={styles.section_head}>
        <button
          type="button"
          className={styles.section_toggle}
          aria-expanded={!folded}
          onClick={() => onToggle(group.key)}
          title={group.path ?? group.name}
        >
          {folded ? (
            <ChevronRight size={14} strokeWidth={2} />
          ) : (
            <ChevronDown size={14} strokeWidth={2} />
          )}
          <Folder size={13} strokeWidth={1.8} />
          <span className={styles.section_label}>{group.name}</span>
          <span className={styles.section_count}>{total}</span>
        </button>
        <span className={styles.group_tail}>
          {group.needsYou > 0 && (
            <span className={styles.needs_badge} data-testid="project-needs-you">
              {t("home.group_needs_you", { count: group.needsYou })}
            </span>
          )}
          {group.path && (
            <button
              type="button"
              className={`${styles.accept_all} ${styles.group_focus}`}
              title={t("home.project_filter_tip")}
              onClick={() => onScopeProject(group.path!)}
            >
              {t("home.group_focus")}
            </button>
          )}
        </span>
      </div>
      {!folded && (
        <>
          {group.rows.map(renderRow)}
          {group.doneRows.length > 0 && (
            <button
              type="button"
              className={styles.done_toggle}
              aria-expanded={showDone}
              onClick={() => setShowDone((v) => !v)}
            >
              {showDone ? (
                <ChevronDown size={12} strokeWidth={2} />
              ) : (
                <ChevronRight size={12} strokeWidth={2} />
              )}
              {t("home.group_done", { count: group.doneRows.length })}
            </button>
          )}
          {showDone && group.doneRows.map(renderRow)}
        </>
      )}
    </section>
  );
}

/** The session a row's context menu acts on. */
export function rowMenuSession(row: HomeRow): SessionInfo | null {
  return row.kind === "card" ? row.session : row.tip;
}
