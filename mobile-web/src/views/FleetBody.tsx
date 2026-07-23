// Mobile renderer for Fleet's MCP control tools (plan/handoff/watch/loop/
// schedule/wiki) — the counterpart of the desktop FleetToolCard. The collapsed
// rail line uses `fleetSummary`; the expanded panel uses `FleetBody`. Parsing
// is shared with the desktop through the copied `fleetTools.ts`.

import type { ReactNode } from "react";
import ReactMarkdown from "react-markdown";
import { mdRemarkPlugins, mdRehypePlugins } from "../markdown/plugins";
import { t } from "../i18n";
import { classifyResult, type FleetResult, type FleetTool } from "./fleetTools";
import styles from "./FleetBody.module.css";

function str(input: Record<string, unknown>, k: string): string {
  return typeof input[k] === "string" ? (input[k] as string) : "";
}

/**
 * Free-text "intent" (note → prompt → text, first non-empty) — the human-
 * readable fields NOT already interpolated into the summary label. Appended to
 * the rail line so the reader sees *what* an op is for without expanding.
 * Mirrors the desktop FleetToolCard's `intentText`.
 */
function intentText(input: Record<string, unknown>): string {
  for (const k of ["note", "prompt", "text"] as const) {
    const v = input[k];
    if (typeof v === "string" && v.trim() !== "") return v.trim();
  }
  return "";
}

/** Collapsed one-line summary shown beside the rail icon: the action label plus
 *  the intent preview (` · …`) when the call carries free-text. */
export function fleetSummary(tool: FleetTool, input: Record<string, unknown>): string {
  const label = fleetSummaryLabel(tool, input);
  const intent = intentText(input);
  return intent ? `${label} · ${intent}` : label;
}

function fleetSummaryLabel(tool: FleetTool, input: Record<string, unknown>): string {
  const action = typeof input.action === "string" ? input.action : "";
  const plan = str(input, "plan_id") || str(input, "plan");
  const task = str(input, "task");
  const id = str(input, "id");
  if (tool === "plan") {
    switch (action) {
      case "check": return t("勾选 {0} · {1}", task, plan);
      case "uncheck": return t("取消勾选 {0} · {1}", task, plan);
      case "create": return t("新建计划 {0}", plan);
      case "add": return t("添加任务 {0} · {1}", task, plan);
      case "resume": return t("接手计划 {0}", plan);
      case "migrate": return t("迁移 TASKS.md");
      case "list": return t("列出计划");
      case "get": return t("查看计划 {0}", plan);
    }
  } else if (tool === "handoff") {
    switch (action) {
      case "register": return t("登记接力");
      case "cancel": return t("取消待定接力");
      case "list": return t("列出接力链");
    }
  } else if (tool === "watch") {
    switch (action) {
      case "create": return t("创建守望");
      case "stop": return t("停止守望 {0}", id);
      case "list": return t("列出守望");
    }
  } else if (tool === "loop") {
    switch (action) {
      case "create": return t("创建循环");
      case "stop": return t("停止循环 {0}", id);
      case "update": return t("更新循环 {0}", id);
      case "get": return t("查看循环 {0}", id);
      case "run": return t("立即运行循环 {0}", id);
      case "list": return t("列出循环");
    }
  } else if (tool === "schedule") {
    switch (action) {
      case "create": return t("创建定时任务");
      case "cancel": return t("取消定时任务 {0}", id);
      case "update": return t("更新定时任务 {0}", id);
      case "get": return t("查看定时任务 {0}", id);
      case "run": return t("立即运行定时任务 {0}", id);
      case "list": return t("列出定时任务");
    }
  } else if (tool === "wiki") {
    switch (action) {
      case "publish": return t("发布 {0}", str(input, "slug"));
      case "cat": return t("查看 {0}", str(input, "slug"));
      case "list": return t("列出知识库");
      case "search": return t("搜索 {0}", str(input, "query"));
    }
  }
  return action || tool;
}

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

/** Relative time — past reuses TasksView's `{0} 分钟前` keys, future adds `…后`. */
function relWhen(ms: number): string {
  const diff = ms - Date.now();
  const abs = Math.abs(diff);
  const future = diff > 0;
  if (abs < 60_000) return t("刚刚");
  if (abs < 3_600_000) {
    const n = Math.floor(abs / 60_000);
    return future ? t("{0} 分钟后", n) : t("{0} 分钟前", n);
  }
  if (abs < 86_400_000) {
    const n = Math.floor(abs / 3_600_000);
    return future ? t("{0} 小时后", n) : t("{0} 小时前", n);
  }
  const n = Math.floor(abs / 86_400_000);
  return future ? t("{0} 天后", n) : t("{0} 天前", n);
}

function fmtDur(secs: number): string {
  if (secs < 60) return t("{0} 秒", secs);
  if (secs < 3600) return t("{0} 分钟", Math.round(secs / 60));
  return t("{0} 小时", Math.round(secs / 3600));
}

function Field({ label, children, mono }: { label: string; children: ReactNode; mono?: boolean }) {
  return (
    <div className={styles.field}>
      <span className={styles.fieldKey}>{label}</span>
      <span className={mono ? styles.fieldValMono : styles.fieldVal}>{children}</span>
    </div>
  );
}

function RecordCard({ title, badge, children }: { title: string; badge?: ReactNode; children: ReactNode }) {
  return (
    <div className={styles.rec}>
      <div className={styles.recHead}>
        <span className={styles.recTitle}>{title}</span>
        {badge}
      </div>
      {children}
    </div>
  );
}

function WatchRecords({ records }: { records: Record<string, unknown>[] }) {
  return (
    <div className={styles.recs}>
      {records.map((r, i) => {
        const until = recStr(r, "untilCmd");
        const note = recStr(r, "note");
        const poll = recNum(r, "pollSecs");
        const deadline = recNum(r, "deadlineAt");
        return (
          <RecordCard key={i} title={note ?? until ?? recStr(r, "id") ?? "watch"}>
            {until && <Field label={t("条件")} mono>{until}</Field>}
            {poll !== undefined && <Field label={t("轮询")}>{fmtDur(poll)}</Field>}
            {deadline !== undefined && <Field label={t("截止")}>{relWhen(deadline)}</Field>}
          </RecordCard>
        );
      })}
    </div>
  );
}

function LoopRecords({ records }: { records: Record<string, unknown>[] }) {
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
            {interval !== undefined && <Field label={t("间隔")}>{fmtDur(interval)}</Field>}
            {done !== undefined && (
              <Field label={t("已运行")}>{max !== undefined ? `${done}/${max}` : `${done}/∞`}</Field>
            )}
            {next !== undefined && <Field label={t("下次")}>{relWhen(next)}</Field>}
          </RecordCard>
        );
      })}
    </div>
  );
}

function ScheduleRecords({ records }: { records: Record<string, unknown>[] }) {
  return (
    <div className={styles.recs}>
      {records.map((r, i) => {
        const prompt = recStr(r, "prompt");
        const status = recStr(r, "status");
        const fireAt = recNum(r, "fireAt");
        const firedAt = recNum(r, "firedAt");
        const fired = status === "fired";
        const badge = status ? (
          <span className={`${styles.statusBadge} ${fired ? styles.statusFired : styles.statusPending}`}>
            {fired ? t("已触发") : t("待触发")}
          </span>
        ) : undefined;
        return (
          <RecordCard key={i} title={prompt ?? recStr(r, "id") ?? "schedule"} badge={badge}>
            {fired && firedAt !== undefined ? (
              <Field label={t("触发于")}>{relWhen(firedAt)}</Field>
            ) : (
              fireAt !== undefined && <Field label={t("触发")}>{relWhen(fireAt)}</Field>
            )}
          </RecordCard>
        );
      })}
    </div>
  );
}

function HandoffRecords({ records }: { records: Record<string, unknown>[] }) {
  return (
    <div className={styles.recs}>
      {records.map((r, i) => {
        const chainId = recStr(r, "chainId");
        const planId = recStr(r, "planId");
        const links = Array.isArray(r.links) ? (r.links as Record<string, unknown>[]) : [];
        const lastNote = links.length ? recStr(links[links.length - 1], "note") : undefined;
        return (
          <RecordCard key={i} title={planId ?? chainId ?? "chain"}>
            <Field label={t("接力")}>{t("{0} 棒", links.length + 1)}</Field>
            {lastNote && <Field label={t("交接")}>{lastNote}</Field>}
          </RecordCard>
        );
      })}
    </div>
  );
}

function RecordsBody({ tool, records }: { tool: FleetTool; records: Record<string, unknown>[] }) {
  switch (tool) {
    case "watch": return <WatchRecords records={records} />;
    case "loop": return <LoopRecords records={records} />;
    case "schedule": return <ScheduleRecords records={records} />;
    case "handoff": return <HandoffRecords records={records} />;
    default: return <pre className={styles.raw}>{JSON.stringify(records, null, 2)}</pre>;
  }
}

function ProgressBar({ done, total }: { done: number; total: number }) {
  const pct = total > 0 ? Math.round((done / total) * 100) : 0;
  return (
    <div className={styles.progress} aria-label={`${done}/${total}`}>
      <div className={styles.progressFill} style={{ width: `${pct}%` }} />
    </div>
  );
}

function ResultView({ result, tool }: { result: FleetResult; tool: FleetTool }) {
  switch (result.kind) {
    case "none":
      return null;
    case "error":
      return <pre className={`${styles.raw} ${styles.rawErr}`}>{result.text}</pre>;
    case "confirm":
      return <div className={styles.confirm}>{result.text}</div>;
    case "plan-list":
      return (
        <div className={styles.planList}>
          {result.plans.map((p) => (
            <div key={p.id} className={styles.planRow}>
              <span className={styles.planId}>{p.id}</span>
              <ProgressBar done={p.done} total={p.total} />
              <span className={styles.planCount}>
                {p.done}/{p.total}
              </span>
            </div>
          ))}
        </div>
      );
    case "plan-get":
      return (
        <div className={styles.checklist}>
          {result.items.map((it, i) => (
            <div key={i} className={`${styles.checkRow} ${it.done ? styles.checkDone : ""}`}>
              <span className={styles.checkMark}>{it.done ? "✓" : "○"}</span>
              <span>{it.text}</span>
            </div>
          ))}
        </div>
      );
    case "wiki-list":
      return (
        <div className={styles.wikiList}>
          {result.docs.map((d, i) => (
            <div key={i} className={styles.wikiRow}>
              <span className={styles.wikiSlug}>{d.slug}</span>
              <span className={styles.wikiKind}>{d.kind}</span>
              <span className={styles.wikiVer}>{d.versions}</span>
              <span className={styles.wikiTitle}>{d.title}</span>
            </div>
          ))}
        </div>
      );
    case "wiki-search":
      return (
        <div className={styles.wikiList}>
          {result.hits.map((h, i) => (
            <div key={i} className={styles.wikiRow}>
              <span className={styles.wikiSlug}>{h.slug}</span>
              <span className={styles.wikiKind}>{h.field}</span>
              <span className={styles.wikiTitle}>{h.matched}</span>
            </div>
          ))}
        </div>
      );
    case "wiki-cat":
      return (
        <div className={styles.markdown}>
          <ReactMarkdown remarkPlugins={mdRemarkPlugins} rehypePlugins={mdRehypePlugins}>
            {result.body}
          </ReactMarkdown>
        </div>
      );
    case "records":
      return <RecordsBody tool={tool} records={result.records} />;
    case "raw":
      return <pre className={styles.raw}>{result.text}</pre>;
    default:
      return null;
  }
}

export function FleetBody({
  tool,
  input,
  content,
  isError,
}: {
  tool: FleetTool;
  input: Record<string, unknown>;
  content: string;
  isError?: boolean;
}) {
  const action = typeof input.action === "string" ? input.action : "";
  const result = classifyResult(tool, action, content, isError ?? false);
  return (
    <div className={styles.body}>
      <ResultView result={result} tool={tool} />
    </div>
  );
}
