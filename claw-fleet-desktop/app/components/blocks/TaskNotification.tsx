import { useState } from "react";
import { useTranslation } from "react-i18next";
import { TextBlock } from "./TextBlock";
import type { PathLinkContext } from "../../markdown/pathLinks";
import styles from "./TaskNotification.module.css";

/**
 * A background agent / subagent completion notice that Claude Code injects as a
 * user turn:
 *
 *   <task-notification>
 *   <task-id>a1389701e02753075</task-id>
 *   <tool-use-id>toolu_…</tool-use-id>
 *   <output-file>/…/a1389701e02753075.output</output-file>
 *   <status>completed</status>
 *   <summary>Agent "审计露馅硬编码 批次3" finished</summary>
 *   <note>A task-notification fires each time this agent stops…</note>
 *   <result>…the agent's final output (markdown)…</result>
 *   </task-notification>
 *
 * Rendered raw, the whole envelope dumps into the bubble as an XML blob. The
 * parse below lifts the fields out so it can render as a card — same pattern as
 * `parseSlashCommand` (command chip) and `isCompactSummary` (compact banner).
 */
export interface ParsedTaskNotification {
  taskId?: string;
  toolUseId?: string;
  outputFile?: string;
  status?: string;
  summary?: string;
  /** Boilerplate that fires every time; intentionally not rendered. */
  note?: string;
  result?: string;
}

export function parseTaskNotification(text: string): ParsedTaskNotification | null {
  if (!text.includes("<task-notification>")) return null;
  const pick = (tag: string): string | undefined => {
    const m = new RegExp(`<${tag}>([\\s\\S]*?)</${tag}>`).exec(text);
    return m ? m[1].trim() : undefined;
  };
  const parsed: ParsedTaskNotification = {
    taskId: pick("task-id"),
    toolUseId: pick("tool-use-id"),
    outputFile: pick("output-file"),
    status: pick("status"),
    summary: pick("summary"),
    note: pick("note"),
    result: pick("result"),
  };
  // Guard against a stray tag with no real payload — fall back to plain text.
  if (!parsed.summary && !parsed.result && !parsed.status) return null;
  return parsed;
}

/** How much of a `summary` we're willing to run through the header's one-line,
 *  nowrap title. Past this it belongs in the body, where it can wrap. */
const TITLE_MAX = 64;

/**
 * An agent notice's `summary` reads like `Agent "审计露馅硬编码 批次3" finished` —
 * the quoted run is the agent's own label and makes a tighter title than the
 * full sentence. A background *shell* notice has no such label: its `stopped`
 * summary is a whole paragraph explaining that no completion record was found.
 * Running that through the title would be unreadable (one nowrap line, most of
 * it ellipsed away), so it goes to the body instead and the title stays generic.
 */
function splitSummary(summary: string | undefined): { title: string | null; body: string | null } {
  if (!summary) return { title: null, body: null };
  const quoted = /["“]([^"”]+)["”]/.exec(summary);
  if (quoted) return { title: quoted[1], body: null };
  if (summary.length <= TITLE_MAX) return { title: summary, body: null };
  return { title: null, body: summary };
}

function basename(path: string): string {
  const i = path.lastIndexOf("/");
  return i >= 0 ? path.slice(i + 1) : path;
}

export function TaskNotification({
  data,
  paths,
}: {
  data: ParsedTaskNotification;
  paths?: PathLinkContext;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(true);

  const raw = (data.status ?? "").toLowerCase();
  const statusCls =
    raw === "completed" || raw === "success" || raw === "done"
      ? styles.status_done
      : raw === "failed" || raw === "error" || raw === "cancelled"
        ? styles.status_error
        : styles.status_neutral;
  const statusLabel =
    raw === "completed" || raw === "success" || raw === "done"
      ? t("detail.task_done", "已完成")
      : raw === "failed" || raw === "error"
        ? t("detail.task_failed", "失败")
        : raw === "cancelled"
          ? t("detail.task_cancelled", "已取消")
          : raw === "stopped"
            ? t("detail.task_stopped", "已停止")
            : data.status;

  const { title, body: summaryBody } = splitSummary(data.summary);
  const bodyText = [summaryBody, data.result].filter(Boolean).join("\n\n");
  const hasBody = !!bodyText;

  return (
    <div className={styles.root}>
      <button
        type="button"
        className={styles.header}
        onClick={() => hasBody && setOpen((o) => !o)}
        // A body-less notice isn't a toggle; don't fake one.
        style={hasBody ? undefined : { cursor: "default" }}
      >
        <span className={styles.icon} aria-hidden>
          🤖
        </span>
        <span className={styles.title}>{title ?? t("detail.task_notification", "后台任务")}</span>
        {statusLabel && (
          <span className={`${styles.badge} ${statusCls}`}>
            <span className={styles.dot} />
            {statusLabel}
          </span>
        )}
        {hasBody && <span className={styles.arrow}>{open ? "▾" : "▸"}</span>}
      </button>

      {hasBody && open && (
        <div className={styles.body}>
          <TextBlock text={bodyText} paths={paths} />
        </div>
      )}

      {(data.outputFile || data.taskId) && (
        <div className={styles.meta}>
          {data.taskId && <span className={styles.meta_item}>{data.taskId.slice(0, 8)}…</span>}
          {data.outputFile && (
            <span className={styles.meta_item} title={data.outputFile}>
              📄 {basename(data.outputFile)}
            </span>
          )}
        </div>
      )}
    </div>
  );
}
