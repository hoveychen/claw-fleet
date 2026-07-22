import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import type { ToolResultBlock, ToolUseBlock as ToolUseBlockType } from "../../types";
import {
  isFleetTool,
  parseFleetCall,
  resultText,
  type FleetResult,
  type FleetView,
} from "./fleetTools";
import styles from "./FleetToolCard.module.css";

/** Fields shown as key/value rows, in this order; anything else falls after. */
const PARAM_ORDER = [
  "plan_id", "plan", "task", "title", "text", "parent", "note", "next",
  "until", "capture", "poll", "timeout", "prompt", "interval", "max",
  "at", "in", "slug", "path", "query", "version", "file", "id", "model", "effort",
];

/** Long free-text fields render as a block rather than an inline value. */
const LONG_FIELDS = new Set(["note", "text", "prompt", "until", "capture", "title"]);

function paramRows(input: Record<string, unknown>): Array<{ key: string; value: string; long: boolean }> {
  const keys = Object.keys(input).filter((k) => k !== "action" && input[k] != null && input[k] !== "");
  keys.sort((a, b) => {
    const ia = PARAM_ORDER.indexOf(a);
    const ib = PARAM_ORDER.indexOf(b);
    return (ia === -1 ? 999 : ia) - (ib === -1 ? 999 : ib);
  });
  return keys.map((k) => {
    const raw = input[k];
    const value = typeof raw === "string" ? raw : JSON.stringify(raw);
    return { key: k, value, long: LONG_FIELDS.has(k) };
  });
}

/** Interpolation vars for the collapsed-header summary line. */
function summaryVars(input: Record<string, unknown>): Record<string, string> {
  const s = (k: string) => (typeof input[k] === "string" ? (input[k] as string) : "");
  return {
    task: s("task"),
    plan: s("plan_id") || s("plan"),
    id: s("id"),
    slug: s("slug"),
    query: s("query"),
    title: s("title"),
  };
}

/** A done/total progress bar for `plan list`. */
function ProgressBar({ done, total }: { done: number; total: number }) {
  const pct = total > 0 ? Math.round((done / total) * 100) : 0;
  return (
    <div className={styles.progress} aria-label={`${done}/${total}`}>
      <div className={styles.progress_fill} style={{ width: `${pct}%` }} />
    </div>
  );
}

function ResultBody({ result }: { result: FleetResult }) {
  switch (result.kind) {
    case "none":
      return null;

    case "error":
      return <pre className={styles.error_text}>{result.text}</pre>;

    case "confirm":
      return <div className={styles.confirm}>{result.text}</div>;

    case "plan-list":
      return (
        <div className={styles.plan_list}>
          {result.plans.map((p) => (
            <div key={p.id} className={styles.plan_row}>
              <span className={styles.plan_id}>{p.id}</span>
              <ProgressBar done={p.done} total={p.total} />
              <span className={styles.plan_count}>
                {p.done}/{p.total}
              </span>
              {p.source && <span className={styles.plan_src} title={p.source}>{p.source}</span>}
            </div>
          ))}
        </div>
      );

    case "plan-get":
      return (
        <div className={styles.checklist}>
          {result.items.map((it, i) => (
            <div key={i} className={`${styles.check_row} ${it.done ? styles.check_done : ""}`}>
              <span className={styles.check_mark}>{it.done ? "✓" : "○"}</span>
              <span className={styles.check_text}>{it.text}</span>
            </div>
          ))}
        </div>
      );

    // P2 精化:wiki-list / wiki-search / wiki-cat / records 目前先以文本兜底,
    // 绝不丢数据。下一 P-task 换成专门渲染。
    case "wiki-list":
      return (
        <pre className={styles.raw_text}>
          {result.docs.map((d) => `${d.slug}  [${d.kind}]  ${d.versions}  ${d.title}`).join("\n")}
        </pre>
      );
    case "wiki-search":
      return (
        <pre className={styles.raw_text}>
          {result.hits.map((h) => `${h.slug}  [${h.field}]  ${h.matched}`).join("\n")}
        </pre>
      );
    case "wiki-cat":
      return <pre className={styles.raw_text}>{result.body}</pre>;
    case "records":
      return <pre className={styles.raw_text}>{JSON.stringify(result.records, null, 2)}</pre>;

    case "raw":
      return <pre className={styles.raw_text}>{result.text}</pre>;

    default:
      return null;
  }
}

interface Props {
  block: ToolUseBlockType;
  result?: ToolResultBlock;
  isPartial?: boolean;
}

/**
 * Inline card for Fleet's MCP control tools (`fleet__plan` etc.). Replaces the
 * generic tool card's `{"action":…}` key/value blob with a structured,
 * human-readable rendering keyed off the tool + action.
 *
 * Collapsed by default with a one-line summary — a long session is mostly tool
 * calls, and an always-open card would dominate it (mirrors `DecisionToolCard`).
 */
export function FleetToolCard({ block, result, isPartial }: Props) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);

  const tool = isFleetTool(block.name);
  const content = result ? resultText(result.content) : "";
  const isError = result?.is_error === true;

  const view: FleetView | null = useMemo(
    () => (tool ? parseFleetCall(tool, block.input, content, isError) : null),
    [tool, block.input, content, isError],
  );

  if (!tool || !view) return null;

  const rows = paramRows(view.input);
  const kindLabel = t(`fleet.kind.${tool}`);
  const summary = t(`fleet.summary.${tool}.${view.action || "unknown"}`, {
    ...summaryVars(view.input),
    defaultValue: view.action || tool,
  });

  return (
    <div className={`${styles.root} ${isError ? styles.root_error : ""}`}>
      <button className={styles.header} onClick={() => setOpen((o) => !o)}>
        <span className={styles.arrow}>{open ? "▾" : "▸"}</span>
        <span className={styles.kind}>{kindLabel}</span>
        <span className={styles.summary}>{summary}</span>
        {isPartial && !result && <span className={styles.spinner}>⟳</span>}
        {isError && <span className={styles.error_badge}>error</span>}
      </button>

      {open && (
        <div className={styles.body}>
          {rows.length > 0 && (
            <div className={styles.params}>
              {rows.map((r) => (
                <div key={r.key} className={r.long ? styles.param_block : styles.param_row}>
                  <span className={styles.param_key}>{t(`fleet.param.${r.key}`, r.key)}</span>
                  <span className={styles.param_val}>{r.value}</span>
                </div>
              ))}
            </div>
          )}
          <ResultBody result={view.result} />
        </div>
      )}
    </div>
  );
}
