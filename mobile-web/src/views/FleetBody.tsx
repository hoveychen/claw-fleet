// Mobile renderer for Fleet's MCP control tools (plan/handoff/watch/loop/
// schedule/wiki) — the counterpart of the desktop FleetToolCard. The collapsed
// rail line uses `fleetSummary`; the expanded panel uses `FleetBody`. Parsing
// is shared with the desktop through the copied `fleetTools.ts`.

import { t } from "../i18n";
import { classifyResult, type FleetResult, type FleetTool } from "./fleetTools";
import styles from "./FleetBody.module.css";

function str(input: Record<string, unknown>, k: string): string {
  return typeof input[k] === "string" ? (input[k] as string) : "";
}

/** Collapsed one-line summary shown beside the rail icon. */
export function fleetSummary(tool: FleetTool, input: Record<string, unknown>): string {
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

function ProgressBar({ done, total }: { done: number; total: number }) {
  const pct = total > 0 ? Math.round((done / total) * 100) : 0;
  return (
    <div className={styles.progress} aria-label={`${done}/${total}`}>
      <div className={styles.progressFill} style={{ width: `${pct}%` }} />
    </div>
  );
}

function ResultView({ result }: { result: FleetResult }) {
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
    // P4 精化:wiki-* / records 目前以文本兜底,绝不丢数据。
    case "wiki-list":
      return (
        <pre className={styles.raw}>
          {result.docs.map((d) => `${d.slug}  [${d.kind}]  ${d.versions}  ${d.title}`).join("\n")}
        </pre>
      );
    case "wiki-search":
      return (
        <pre className={styles.raw}>
          {result.hits.map((h) => `${h.slug}  [${h.field}]  ${h.matched}`).join("\n")}
        </pre>
      );
    case "wiki-cat":
      return <pre className={styles.raw}>{result.body}</pre>;
    case "records":
      return <pre className={styles.raw}>{JSON.stringify(result.records, null, 2)}</pre>;
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
      <ResultView result={result} />
    </div>
  );
}
