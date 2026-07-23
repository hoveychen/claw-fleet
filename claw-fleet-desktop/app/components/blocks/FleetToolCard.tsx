import { useMemo, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import ReactMarkdown from "react-markdown";
import { safeMarkdownComponents, safeRemarkPlugins, safeRehypePlugins } from "../../markdown/safeLinks";
import { normalizeSvgBlankLines } from "../../markdown/plugins";
import { formatMsgTime } from "../../messageRows";
import type { ToolResultBlock, ToolUseBlock as ToolUseBlockType } from "../../types";
import {
  isFleetTool,
  parseFleetCall,
  resultText,
  type FleetResult,
  type FleetTool,
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

/**
 * Free-text "intent" surfaced on the collapsed header, right after the summary
 * label, so the reader sees *what* an op is for without expanding the card.
 * These are exactly the human-readable fields NOT already interpolated into the
 * summary via `summaryVars` (note → watch/handoff, prompt → loop/schedule,
 * text → plan add). First non-empty wins.
 */
function intentText(input: Record<string, unknown>): string {
  for (const k of ["note", "prompt", "text"] as const) {
    const v = input[k];
    if (typeof v === "string" && v.trim() !== "") return v.trim();
  }
  return "";
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

// ── record-field helpers (handoff/watch/loop/schedule JSON records) ──────────
function recStr(rec: Record<string, unknown>, key: string): string | undefined {
  const v = rec[key];
  if (typeof v === "string") return v;
  if (typeof v === "number") return String(v);
  return undefined;
}
function recNum(rec: Record<string, unknown>, key: string): number | undefined {
  const v = rec[key];
  return typeof v === "number" && Number.isFinite(v) ? v : undefined;
}

/** Relative time. Past reuses the shared `*_ago` keys; future uses fleet.time.in_*. */
function relWhen(ms: number, t: TFunction): { label: string; title: string } {
  const diff = ms - Date.now();
  const abs = Math.abs(diff);
  const future = diff > 0;
  let label: string;
  if (abs < 60_000) label = t("just_now");
  else if (abs < 3_600_000) {
    const n = Math.floor(abs / 60_000);
    label = future ? t("fleet.time.in_m", { n }) : t("m_ago", { n });
  } else if (abs < 86_400_000) {
    const n = Math.floor(abs / 3_600_000);
    label = future ? t("fleet.time.in_h", { n }) : t("h_ago", { n });
  } else {
    const n = Math.floor(abs / 86_400_000);
    label = future ? t("fleet.time.in_d", { n }) : t("d_ago", { n });
  }
  return { label, title: formatMsgTime(new Date(ms).toISOString())?.full ?? "" };
}

/** Human duration for interval/poll seconds. */
function fmtDur(secs: number, t: TFunction): string {
  if (secs < 60) return t("fleet.dur.s", { n: secs });
  if (secs < 3600) return t("fleet.dur.m", { n: Math.round(secs / 60) });
  return t("fleet.dur.h", { n: Math.round(secs / 3600) });
}

function Field({ label, children, mono }: { label: string; children: ReactNode; mono?: boolean }) {
  return (
    <div className={styles.field}>
      <span className={styles.field_key}>{label}</span>
      <span className={mono ? styles.field_val_mono : styles.field_val}>{children}</span>
    </div>
  );
}

function RecordCard({ title, badge, children }: { title: string; badge?: ReactNode; children: ReactNode }) {
  return (
    <div className={styles.rec}>
      <div className={styles.rec_head}>
        <span className={styles.rec_title} title={title}>{title}</span>
        {badge}
      </div>
      {children}
    </div>
  );
}

function Relative({ ms, t }: { ms: number; t: TFunction }) {
  const { label, title } = relWhen(ms, t);
  return <span title={title}>{label}</span>;
}

/** watch list → WatchRecord[] */
function WatchRecords({ records, t }: { records: Record<string, unknown>[]; t: TFunction }) {
  return (
    <div className={styles.recs}>
      {records.map((r, i) => {
        const until = recStr(r, "untilCmd");
        const note = recStr(r, "note");
        const poll = recNum(r, "pollSecs");
        const deadline = recNum(r, "deadlineAt");
        return (
          <RecordCard key={i} title={note ?? until ?? recStr(r, "id") ?? "watch"}>
            {until && <Field label={t("fleet.rec.until")} mono>{until}</Field>}
            {poll !== undefined && <Field label={t("fleet.rec.poll")}>{fmtDur(poll, t)}</Field>}
            {deadline !== undefined && (
              <Field label={t("fleet.rec.deadline")}>
                <Relative ms={deadline} t={t} />
              </Field>
            )}
          </RecordCard>
        );
      })}
    </div>
  );
}

/** loop list/get → LoopRecord[] */
function LoopRecords({ records, t }: { records: Record<string, unknown>[]; t: TFunction }) {
  return (
    <div className={styles.recs}>
      {records.map((r, i) => {
        const prompt = recStr(r, "prompt");
        const interval = recNum(r, "intervalSecs");
        const done = recNum(r, "iterationsDone");
        const max = recNum(r, "maxIterations");
        const next = recNum(r, "nextFireAt");
        return (
          <RecordCard key={i} title={prompt ?? recStr(r, "id") ?? "loop"}>
            {interval !== undefined && <Field label={t("fleet.rec.interval")}>{fmtDur(interval, t)}</Field>}
            {done !== undefined && (
              <Field label={t("fleet.rec.iterations")}>{max !== undefined ? `${done}/${max}` : `${done}/∞`}</Field>
            )}
            {next !== undefined && (
              <Field label={t("fleet.rec.next")}>
                <Relative ms={next} t={t} />
              </Field>
            )}
          </RecordCard>
        );
      })}
    </div>
  );
}

/** schedule list/get → ScheduleRecord[] */
function ScheduleRecords({ records, t }: { records: Record<string, unknown>[]; t: TFunction }) {
  return (
    <div className={styles.recs}>
      {records.map((r, i) => {
        const prompt = recStr(r, "prompt");
        const status = recStr(r, "status");
        const fireAt = recNum(r, "fireAt");
        const firedAt = recNum(r, "firedAt");
        const fired = status === "fired";
        const badge = status ? (
          <span className={`${styles.status_badge} ${fired ? styles.status_fired : styles.status_pending}`}>
            {t(`fleet.status.${status}`, status)}
          </span>
        ) : undefined;
        return (
          <RecordCard key={i} title={prompt ?? recStr(r, "id") ?? "schedule"} badge={badge}>
            {fired && firedAt !== undefined ? (
              <Field label={t("fleet.rec.fired")}>
                <Relative ms={firedAt} t={t} />
              </Field>
            ) : (
              fireAt !== undefined && (
                <Field label={t("fleet.rec.fire")}>
                  <Relative ms={fireAt} t={t} />
                </Field>
              )
            )}
          </RecordCard>
        );
      })}
    </div>
  );
}

/** handoff list → HandoffChain[] */
function HandoffRecords({ records, t }: { records: Record<string, unknown>[]; t: TFunction }) {
  return (
    <div className={styles.recs}>
      {records.map((r, i) => {
        const chainId = recStr(r, "chainId");
        const planId = recStr(r, "planId");
        const links = Array.isArray(r.links) ? (r.links as Record<string, unknown>[]) : [];
        const hops = links.length + 1;
        const lastNote = links.length ? recStr(links[links.length - 1], "note") : undefined;
        return (
          <RecordCard key={i} title={planId ?? chainId ?? "chain"}>
            <Field label={t("fleet.rec.hops")}>{t("fleet.rec.hops_n", { n: hops })}</Field>
            {lastNote && <Field label={t("fleet.rec.note")}>{lastNote}</Field>}
          </RecordCard>
        );
      })}
    </div>
  );
}

function RecordsBody({ tool, records, t }: { tool: FleetTool; records: Record<string, unknown>[]; t: TFunction }) {
  switch (tool) {
    case "watch": return <WatchRecords records={records} t={t} />;
    case "loop": return <LoopRecords records={records} t={t} />;
    case "schedule": return <ScheduleRecords records={records} t={t} />;
    case "handoff": return <HandoffRecords records={records} t={t} />;
    default: return <pre className={styles.raw_text}>{JSON.stringify(records, null, 2)}</pre>;
  }
}

function ResultBody({ result, tool }: { result: FleetResult; tool: FleetTool }) {
  const { t } = useTranslation();
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

    case "wiki-list":
      return (
        <div className={styles.wiki_list}>
          {result.docs.map((d, i) => (
            <div key={i} className={styles.wiki_row}>
              <span className={styles.wiki_slug}>{d.slug}</span>
              <span className={styles.wiki_kind}>{d.kind}</span>
              <span className={styles.wiki_ver}>{d.versions}</span>
              <span className={styles.wiki_title}>{d.title}</span>
            </div>
          ))}
        </div>
      );
    case "wiki-search":
      return (
        <div className={styles.wiki_list}>
          {result.hits.map((h, i) => (
            <div key={i} className={styles.wiki_row}>
              <span className={styles.wiki_slug}>{h.slug}</span>
              <span className={styles.wiki_kind}>{h.field}</span>
              <span className={styles.wiki_title}>{h.matched}</span>
            </div>
          ))}
        </div>
      );
    case "wiki-cat":
      return (
        <div className={styles.markdown}>
          <ReactMarkdown
            remarkPlugins={safeRemarkPlugins}
            rehypePlugins={safeRehypePlugins}
            components={safeMarkdownComponents}
          >
            {normalizeSvgBlankLines(result.body)}
          </ReactMarkdown>
        </div>
      );
    case "records":
      return <RecordsBody tool={tool} records={result.records} t={t} />;

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
  const intent = intentText(view.input);

  return (
    <div className={`${styles.root} ${isError ? styles.root_error : ""}`}>
      <button className={styles.header} onClick={() => setOpen((o) => !o)}>
        <span className={styles.arrow}>{open ? "▾" : "▸"}</span>
        <span className={styles.kind}>{kindLabel}</span>
        <span className={styles.summary}>{summary}</span>
        {intent && <span className={styles.intent} title={intent}>{intent}</span>}
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
          <ResultBody result={view.result} tool={view.tool} />
        </div>
      )}
    </div>
  );
}
