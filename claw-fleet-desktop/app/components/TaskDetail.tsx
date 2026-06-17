// TaskDetail — the rebuilt task detail view (tasks-ui-rebuild P5).
//
// Layout: a header (breadcrumb / editable title / status / start-accept
// actions) over a two-column body:
//   - main column: 来龙去脉 timeline → DAG plan visualization → Deliverables
//   - right rail: progress ring + time + token + cost + current action
//
// All numbers are real: deliverables come from `get_task_deliverables` (git
// diff stat), token/cost from `get_task_token_breakdown` resolved via the
// master session's jsonl path. Anything we can't resolve renders "—" rather
// than a fabricated value.

import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { ArrowLeft, GitBranch, Eye, FileText, Plus, Minus, Trash2 } from "lucide-react";
import styles from "./TaskDetail.module.css";
import { useSessionsStore, type Project } from "../store";
import { useRuntimeTasksStore } from "../runtimeTasksStore";
import type { AcceptMode, PItem, PItemStatus, Task, TaskStatus, TaskDeliverables, TaskTokenBreakdown } from "../types";
import { pItemStatusKey } from "../types";

// ── shared helpers ──────────────────────────────────────────────────────────

function statusColor(status: TaskStatus): string {
  switch (status) {
    case "running": return "#f0875a";
    case "reviewing":
    case "awaitingAcceptance": return "#7aa0fa";
    case "paused": return "#d0a85a";
    case "drafting": return "#9a9aa0";
    case "done": return "#5ac88c";
    default: return "#6b6b72";
  }
}

function pItemColor(status: PItemStatus): string {
  const k = pItemStatusKey(status);
  switch (k) {
    case "running": return "#f0875a";
    case "reviewing":
    case "waitHumanGate": return "#7aa0fa";
    case "done": return "#5ac88c";
    case "skipped": return "#9a9aa0";
    case "failed": return "#ef6b6b";
    default: return "#454549"; // waitDeps
  }
}

function fmtDuration(secs: number): string {
  if (secs < 0) secs = 0;
  if (secs < 60) return `${Math.round(secs)}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ${Math.round(secs % 60)}s`;
  if (secs < 86400) return `${Math.floor(secs / 3600)}h ${Math.round((secs % 3600) / 60)}m`;
  return `${Math.floor(secs / 86400)}d ${Math.round((secs % 86400) / 3600)}h`;
}

function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(n);
}

function fmtClock(secs?: number | null): string {
  if (!secs) return "";
  const d = new Date(secs * 1000);
  const mm = `${d.getMonth() + 1}`.padStart(2, "0");
  const dd = `${d.getDate()}`.padStart(2, "0");
  const hh = `${d.getHours()}`.padStart(2, "0");
  const mi = `${d.getMinutes()}`.padStart(2, "0");
  return `${mm}-${dd} ${hh}:${mi}`;
}

function summarisePlan(task: Task) {
  let total = 0, done = 0, failed = 0, running = 0;
  for (const p of Object.values(task.plan.items)) {
    total += 1;
    const k = pItemStatusKey(p.status);
    if (k === "done" || k === "skipped") done += 1;
    else if (k === "failed") failed += 1;
    else if (k === "running" || k === "reviewing" || k === "waitHumanGate") running += 1;
  }
  return { total, done, failed, running };
}

// ── root ──────────────────────────────────────────────────────────────────

export function TaskDetail({
  task,
  project,
  onBack,
  onStart,
  onAccept,
  onRerunE2e,
  onRename,
  onDelete,
}: {
  task: Task;
  project: Project | null;
  onBack: () => void;
  onStart: () => Promise<void>;
  onAccept: (mode: AcceptMode) => Promise<void>;
  onRerunE2e: () => Promise<void>;
  onRename: (title: string) => Promise<void>;
  onDelete: () => Promise<void>;
}) {
  return (
    <div className={styles.root}>
      <DetailHeader task={task} project={project} onBack={onBack} onStart={onStart} onAccept={onAccept} onRename={onRename} onDelete={onDelete} />
      <div className={styles.layout}>
        <div className={styles.main}>
          <E2eBanner task={task} onRerunE2e={onRerunE2e} />
          <Timeline task={task} />
          <PlanKanban task={task} />
          <Deliverables task={task} />
        </div>
        <MetricsRail task={task} />
      </div>
    </div>
  );
}

// ── header ──────────────────────────────────────────────────────────────────

function DetailHeader({
  task, project, onBack, onStart, onAccept, onRename, onDelete,
}: {
  task: Task;
  project: Project | null;
  onBack: () => void;
  onStart: () => Promise<void>;
  onAccept: (mode: AcceptMode) => Promise<void>;
  onRename: (title: string) => Promise<void>;
  onDelete: () => Promise<void>;
}) {
  const { t } = useTranslation();
  const isLive = useRuntimeTasksStore((s) => Boolean(s.byTaskId[task.id]));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const canStart = !busy && task.status === "drafting";
  const canAccept = !busy && task.status === "awaitingAcceptance";

  const run = async (fn: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try { await fn(); }
    catch (e) { setError(String((e as { message?: string })?.message ?? e)); }
    finally { setBusy(false); }
  };

  return (
    <>
      <div className={styles.header}>
        <button className={styles.back_btn} onClick={onBack} aria-label={t("tasks.back_to_list", "Back")}>
          <ArrowLeft size={16} strokeWidth={1.7} />
        </button>
        <div className={styles.header_titles}>
          <div className={styles.breadcrumb}>
            {project?.name ?? t("tasks.unknown_project", "Unknown project")}
          </div>
          <div className={styles.title_row}>
            <EditableTitle task={task} onRename={onRename} />
            <span className={styles.status_badge} style={{ color: statusColor(task.status), borderColor: statusColor(task.status) }}>
              {t(`tasks.status.${task.status}`, task.status)}
            </span>
            {isLive && <span className={styles.live_dot} title="live" />}
          </div>
        </div>
        <div className={styles.header_actions}>
          {task.status === "awaitingAcceptance" ? (
            <>
              <button
                className={styles.primary_btn}
                onClick={() => run(() => onAccept("mergeBack"))}
                disabled={!canAccept}
                title={t("tasks.accept_merge_hint", "Merge the task branch back into its base branch, then delete it")}
              >
                {busy ? t("tasks.accepting", "Accepting…") : `✓ ${t("tasks.accept_merge", "Accept & merge")}`}
              </button>
              <button
                className={styles.cancel_btn}
                onClick={() => run(() => onAccept("keepBranch"))}
                disabled={!canAccept}
                title={t("tasks.accept_keep_hint", "Accept without merging — keep the branch to merge / PR yourself")}
              >
                {t("tasks.accept_keep", "Keep branch")}
              </button>
            </>
          ) : (
            <button className={styles.primary_btn} onClick={() => run(onStart)} disabled={!canStart}>
              {busy ? t("tasks.starting", "Starting…") : `▶ ${t("tasks.start", "Start")}`}
            </button>
          )}
          {confirmDelete ? (
            <div className={styles.confirm_row}>
              <span className={styles.confirm_text}>{t("tasks.delete_confirm", "Delete this task?")}</span>
              <button
                className={styles.danger_btn}
                disabled={busy}
                onClick={() => run(async () => { await onDelete(); })}
              >
                {busy ? t("tasks.deleting", "Deleting…") : t("tasks.delete", "Delete")}
              </button>
              <button className={styles.cancel_btn} disabled={busy} onClick={() => setConfirmDelete(false)}>
                {t("cancel", "Cancel")}
              </button>
            </div>
          ) : (
            <button
              className={styles.delete_btn}
              onClick={() => setConfirmDelete(true)}
              title={t("tasks.delete", "Delete")}
              aria-label={t("tasks.delete", "Delete")}
            >
              <Trash2 size={15} strokeWidth={1.7} />
            </button>
          )}
        </div>
      </div>
      {task.taskBranch && (
        <div className={styles.branch_line}>
          <GitBranch size={12} strokeWidth={1.6} />
          <code>{task.taskBranch}</code>
        </div>
      )}
      {error && <div className={styles.error}>{t("tasks.start_failed", "Start failed: {{message}}", { message: error })}</div>}
    </>
  );
}

function EditableTitle({ task, onRename }: { task: Task; onRename: (title: string) => Promise<void> }) {
  const { t } = useTranslation();
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(task.title);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => { if (!editing) setDraft(task.title); }, [task.title, editing]);
  useEffect(() => { if (editing) { inputRef.current?.focus(); inputRef.current?.select(); } }, [editing]);

  const commit = async () => {
    const next = draft.trim();
    if (!next || next === task.title) { setEditing(false); setDraft(task.title); return; }
    try { await onRename(next); } catch { setDraft(task.title); } finally { setEditing(false); }
  };

  if (editing) {
    return (
      <input
        ref={inputRef}
        className={styles.title_input}
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") { e.preventDefault(); void commit(); }
          else if (e.key === "Escape") { e.preventDefault(); setDraft(task.title); setEditing(false); }
        }}
      />
    );
  }
  return (
    <h1 className={styles.title} onClick={() => setEditing(true)} title={t("tasks.click_to_rename", "Click to rename")}>
      {task.title}
      {task.titleAuto && <span className={styles.title_auto}>✎</span>}
    </h1>
  );
}

// ── e2e verification banner ───────────────────────────────────────────────────

// Surfaces the task-level e2e outcome. A FAILED run is why a task can sit in
// `running` with every P-item done but no acceptance prompt — without this the
// UI would be silent about it.
function E2eBanner({ task, onRerunE2e }: { task: Task; onRerunE2e: () => Promise<void> }) {
  const { t } = useTranslation();
  const [rerunning, setRerunning] = useState(false);
  const e2e = task.e2e;
  if (!e2e) return null;
  if (e2e.passed) {
    return (
      <div className={styles.e2e_ok}>
        ✓ {t("detail.e2e_passed", "End-to-end verification passed")}
        <code className={styles.e2e_cmd}>{e2e.command}</code>
      </div>
    );
  }
  const rerun = async () => {
    setRerunning(true);
    try {
      await onRerunE2e();
    } finally {
      setRerunning(false);
    }
  };
  return (
    <div className={styles.e2e_fail}>
      <div className={styles.e2e_fail_head}>
        ⚠ {t("detail.e2e_failed", "End-to-end verification failed — task held before acceptance")}
      </div>
      <code className={styles.e2e_cmd}>{e2e.command}</code>
      {e2e.gaps.length > 0 && (
        <ul className={styles.e2e_gaps}>
          {e2e.gaps.map((g, i) => (
            <li key={i}>{g}</li>
          ))}
        </ul>
      )}
      <button className={styles.e2e_rerun} onClick={rerun} disabled={rerunning}>
        {rerunning ? t("detail.e2e_rerunning", "Re-running…") : t("detail.e2e_rerun", "Re-run e2e")}
      </button>
    </div>
  );
}

// ── timeline (来龙去脉) ───────────────────────────────────────────────────────

interface TLEvent { time: number; title: string; detail?: string; done: boolean }

function buildTimeline(task: Task, t: (k: string, d: string, o?: Record<string, unknown>) => string): TLEvent[] {
  const events: TLEvent[] = [];
  const items = Object.values(task.plan.items);

  const matCount = task.inboxMaterials?.length ?? 0;
  events.push({
    time: task.createdAt,
    title: t("detail.tl_created", "Task created"),
    detail: matCount > 0 ? t("detail.tl_materials", "{{count}} material(s) attached", { count: matCount }) : undefined,
    done: true,
  });

  if (task.startedAt) {
    events.push({
      time: task.startedAt,
      title: t("detail.tl_started", "Task started"),
      detail: task.taskBranch ? t("detail.tl_branch", "branch {{branch}}", { branch: task.taskBranch }) : undefined,
      done: true,
    });
    if (items.length > 0) {
      events.push({
        time: task.startedAt + 1,
        title: t("detail.tl_planned", "Plan generated — {{count}} P-items", { count: items.length }),
        done: true,
      });
    }
  }

  for (const p of items) {
    if (p.startedAt) {
      events.push({ time: p.startedAt, title: t("detail.tl_pitem_start", "{{id}} started", { id: p.id }), detail: p.desc, done: true });
    }
    if (p.completedAt) {
      const k = pItemStatusKey(p.status);
      const word = k === "failed"
        ? t("detail.tl_failed", "failed")
        : k === "skipped"
          ? t("detail.tl_skipped", "skipped")
          : t("detail.tl_done", "done");
      events.push({
        time: p.completedAt,
        title: t("detail.tl_pitem_end", "{{id}} {{word}}", { id: p.id, word }),
        detail: p.outputSummary ?? undefined,
        done: true,
      });
    }
  }

  if (task.completedAt) {
    events.push({ time: task.completedAt, title: t("detail.tl_completed", "Task completed"), done: true });
  }

  return events.sort((a, b) => a.time - b.time);
}

function Timeline({ task }: { task: Task }) {
  const { t } = useTranslation();
  const events = useMemo(() => buildTimeline(task, t as never), [task, t]);
  if (events.length === 0) return null;
  return (
    <section className={styles.section}>
      <h3 className={styles.section_title}>{t("detail.section_timeline", "来龙去脉")}</h3>
      <div className={styles.timeline}>
        {events.map((e, i) => (
          <div key={i} className={`${styles.tl_item} ${e.done ? styles.tl_done : ""}`}>
            <div className={styles.tl_title}>{e.title}</div>
            <div className={styles.tl_meta}>
              {fmtClock(e.time)}
              {e.detail && <span className={styles.tl_detail}> · {e.detail}</span>}
            </div>
          </div>
        ))}
      </div>
    </section>
  );
}

// ── P-item kanban (Pending / Active / Resolved) ──────────────────────────────

function PlanKanban({ task }: { task: Task }) {
  const { t } = useTranslation();
  const items = Object.values(task.plan.items);
  if (items.length === 0) {
    return (
      <section className={styles.section}>
        <h3 className={styles.section_title}>{t("detail.section_plan", "计划")}</h3>
        <div className={styles.no_plan}>
          {task.status === "running"
            ? t("tasks.planning_in_progress", "Planning session is clarifying the requirements — the plan appears here once it's written.")
            : t("tasks.no_plan_yet", "No plan yet. Start the task to launch the interactive planning session.")}
        </div>
      </section>
    );
  }

  // Column buckets:
  //  - pending : waitDeps
  //  - active  : running / reviewing / waitHumanGate
  //  - resolved: done / skipped / failed
  const pending: PItem[] = [];
  const active: PItem[] = [];
  const resolved: PItem[] = [];
  for (const p of items) {
    const k = pItemStatusKey(p.status);
    if (k === "waitDeps") pending.push(p);
    else if (k === "running" || k === "reviewing" || k === "waitHumanGate") active.push(p);
    else resolved.push(p);
  }
  for (const arr of [pending, active, resolved]) arr.sort((a, b) => a.id.localeCompare(b.id));

  // Dependency layers for the DAG: level = longest dependency chain.
  const levels = computeLevels(task.plan.items);
  const maxLevel = levels.size > 0 ? Math.max(...Array.from(levels.values())) : 0;
  const columns: PItem[][] = Array.from({ length: maxLevel + 1 }, () => []);
  for (const id of Object.keys(task.plan.items)) columns[levels.get(id) ?? 0].push(task.plan.items[id]);
  for (const col of columns) col.sort((a, b) => a.id.localeCompare(b.id));

  return (
    <section className={styles.section}>
      <h3 className={styles.section_title}>{t("detail.section_plan", "计划")}</h3>

      {/* Kanban — status distribution at a glance */}
      <div className={styles.subhead}>{t("detail.plan_kanban", "看板")}</div>
      <div className={styles.kanban}>
        <KanbanColumn title={t("tasks.col_pending", "Pending")} items={pending} />
        <KanbanColumn title={t("tasks.col_active", "Active")} items={active} highlight />
        <KanbanColumn title={t("tasks.col_resolved", "Resolved")} items={resolved} />
      </div>

      {/* DAG — dependency structure */}
      <div className={styles.subhead}>{t("detail.plan_dag", "依赖图")}</div>
      <div className={styles.dag}>
        {columns.map((col, ci) => (
          <div key={ci} className={styles.dag_col_wrap}>
            <div className={styles.dag_col}>
              {col.map((p) => (
                <div
                  key={p.id}
                  className={styles.dag_node}
                  style={{ borderLeftColor: pItemColor(p.status) }}
                  title={`${p.id} — ${pItemStatusKey(p.status)}\n${p.desc}`}
                >
                  <div className={styles.dag_node_head}>
                    <span className={styles.dag_dot} style={{ background: pItemColor(p.status) }} />
                    <span className={styles.dag_id}>{p.id}</span>
                    {p.humanGate && <Eye size={11} strokeWidth={1.6} className={styles.dag_gate} />}
                  </div>
                  <div className={styles.dag_desc}>{p.desc}</div>
                </div>
              ))}
            </div>
            {ci < columns.length - 1 && <div className={styles.dag_arrow}>→</div>}
          </div>
        ))}
      </div>

      <FailureReasons items={items} />
    </section>
  );
}

// Lists why each rejected P-item failed (persisted failure_gaps) — so a red node
// in the kanban/DAG is explained instead of leaving the user guessing.
function FailureReasons({ items }: { items: PItem[] }) {
  const { t } = useTranslation();
  const failed = items.filter(
    (p) => pItemStatusKey(p.status) === "failed" && (p.failureGaps?.length ?? 0) > 0,
  );
  if (failed.length === 0) return null;
  return (
    <div className={styles.failures}>
      <div className={styles.subhead}>{t("detail.plan_failures", "失败原因")}</div>
      {failed.map((p) => (
        <div key={p.id} className={styles.failure_item}>
          <span className={styles.failure_id}>{p.id}</span>
          <ul className={styles.failure_gaps}>
            {(p.failureGaps ?? []).map((g, i) => (
              <li key={i}>{g}</li>
            ))}
          </ul>
        </div>
      ))}
    </div>
  );
}

function computeLevels(items: Record<string, PItem>): Map<string, number> {
  const level = new Map<string, number>();
  const visiting = new Set<string>();
  const depth = (id: string): number => {
    if (level.has(id)) return level.get(id)!;
    if (visiting.has(id)) return 0; // cycle guard
    visiting.add(id);
    const deps = items[id]?.dependsOn ?? [];
    const d = deps.length === 0 ? 0 : 1 + Math.max(...deps.map((dep) => (items[dep] ? depth(dep) : -1)));
    visiting.delete(id);
    level.set(id, d);
    return d;
  };
  for (const id of Object.keys(items)) depth(id);
  return level;
}

function KanbanColumn({ title, items, highlight }: { title: string; items: PItem[]; highlight?: boolean }) {
  return (
    <div className={`${styles.kb_col} ${highlight ? styles.kb_col_active : ""}`}>
      <div className={styles.kb_col_head}>
        <span>{title}</span>
        <span className={styles.kb_count}>{items.length}</span>
      </div>
      <div className={styles.kb_col_body}>
        {items.length === 0 && <div className={styles.kb_empty}>—</div>}
        {items.map((p) => (
          <PItemCard key={p.id} pitem={p} />
        ))}
      </div>
    </div>
  );
}

function PItemCard({ pitem }: { pitem: PItem }) {
  const { t } = useTranslation();
  const k = pItemStatusKey(pitem.status);
  return (
    <div className={styles.kb_card} style={{ borderLeftColor: pItemColor(pitem.status) }}>
      <div className={styles.kb_card_head}>
        <span className={styles.kb_dot} style={{ background: pItemColor(pitem.status) }} />
        <span className={styles.kb_id}>{pitem.id}</span>
        {pitem.humanGate && <Eye size={11} strokeWidth={1.6} className={styles.kb_gate} />}
      </div>
      <div className={styles.kb_desc}>{pitem.desc}</div>
      {k === "waitHumanGate" && (
        <div className={styles.kb_gate_pending}>
          {t("tasks.waiting_for_user_review", "等待用户审核 — 请在 Decision Panel 处理")}
        </div>
      )}
      {pitem.touches.length > 0 && (
        <div className={styles.kb_touches}>
          {pitem.touches.slice(0, 3).map((f) => (
            <code key={f}>{f}</code>
          ))}
          {pitem.touches.length > 3 && <span>+{pitem.touches.length - 3}</span>}
        </div>
      )}
    </div>
  );
}

// ── deliverables ────────────────────────────────────────────────────────────

function Deliverables({ task }: { task: Task }) {
  const { t } = useTranslation();
  const [data, setData] = useState<TaskDeliverables | null>(null);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    let cancelled = false;
    if (!task.taskBranch) { setData(null); return; }
    setLoading(true);
    invoke<TaskDeliverables>("get_task_deliverables", { taskId: task.id })
      .then((d) => { if (!cancelled) setData(d); })
      .catch(() => { if (!cancelled) setData(null); })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [task.id, task.taskBranch, task.completedAt]);

  const summaries = useMemo(
    () => Object.values(task.plan.items).filter((p) => p.outputSummary).map((p) => ({ id: p.id, summary: p.outputSummary! })),
    [task.plan.items],
  );

  return (
    <section className={styles.section}>
      <h3 className={styles.section_title}>{t("detail.section_deliverables", "Deliverables")}</h3>

      {!task.taskBranch ? (
        <div className={styles.no_plan}>{t("detail.deliverables_none", "No deliverables yet — start the task to produce changes.")}</div>
      ) : (
        <>
          {data && data.files.length > 0 && (
            <div className={styles.deliv_totals}>
              <span className={styles.deliv_add}>+{data.totalAdditions}</span>
              <span className={styles.deliv_del}>−{data.totalDeletions}</span>
              <span className={styles.deliv_count}>{data.files.length} {t("detail.files", "files")}</span>
            </div>
          )}
          <div className={styles.deliv_files}>
            {loading && <div className={styles.muted}>{t("loading", "Loading…")}</div>}
            {!loading && data && data.files.length === 0 && (
              <div className={styles.muted}>{t("detail.deliverables_empty", "No file changes on the branch yet.")}</div>
            )}
            {data?.files.map((f) => (
              <div key={f.path} className={styles.deliv_row}>
                <FileText size={12} strokeWidth={1.5} className={styles.deliv_icon} />
                <code className={styles.deliv_path} title={f.path}>{f.path}</code>
                {f.binary ? (
                  <span className={styles.deliv_bin}>bin</span>
                ) : (
                  <span className={styles.deliv_stat}>
                    <span className={styles.deliv_add}><Plus size={9} strokeWidth={2.2} />{f.additions}</span>
                    <span className={styles.deliv_del}><Minus size={9} strokeWidth={2.2} />{f.deletions}</span>
                  </span>
                )}
              </div>
            ))}
          </div>
        </>
      )}

      {summaries.length > 0 && (
        <div className={styles.deliv_summaries}>
          <div className={styles.deliv_sub_title}>{t("detail.output_summaries", "Output summaries")}</div>
          {summaries.map((s) => (
            <div key={s.id} className={styles.deliv_summary}>
              <span className={styles.deliv_summary_id}>{s.id}</span>
              <span className={styles.deliv_summary_text}>{s.summary}</span>
            </div>
          ))}
        </div>
      )}
    </section>
  );
}

// ── metrics rail ──────────────────────────────────────────────────────────

function MetricsRail({ task }: { task: Task }) {
  const { t } = useTranslation();
  const summary = useMemo(() => summarisePlan(task), [task]);
  const pct = summary.total > 0 ? Math.round((summary.done / summary.total) * 100) : 0;

  // Elapsed time: started → completed (or now for in-flight tasks).
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));
  useEffect(() => {
    if (task.status !== "running" && task.status !== "reviewing") return;
    const id = setInterval(() => setNow(Math.floor(Date.now() / 1000)), 1000);
    return () => clearInterval(id);
  }, [task.status]);
  const elapsed = task.startedAt ? (task.completedAt ?? now) - task.startedAt : null;

  // Token / cost via the master session's token breakdown.
  const sessions = useSessionsStore((s) => s.sessions);
  const masterJsonl = useMemo(
    () => sessions.find((s) => s.id === task.masterSessionId)?.jsonlPath ?? null,
    [sessions, task.masterSessionId],
  );
  const [breakdown, setBreakdown] = useState<TaskTokenBreakdown | null>(null);
  useEffect(() => {
    let cancelled = false;
    if (!masterJsonl) { setBreakdown(null); return; }
    invoke<TaskTokenBreakdown>("get_task_token_breakdown", { jsonlPath: masterJsonl, projectRoot: null })
      .then((b) => { if (!cancelled) setBreakdown(b); })
      .catch(() => { if (!cancelled) setBreakdown(null); });
    return () => { cancelled = true; };
  }, [masterJsonl, task.completedAt]);

  const outTok = breakdown ? breakdown.totalsUsage.outputTokens : null;
  const inTok = breakdown ? breakdown.totalsUsage.inputTokens : null;
  const cost = breakdown?.totalsEstimatedCostUsd ?? null;

  const currentAction =
    task.status === "running" ? t("detail.action_running", "Executing P-items")
    : task.status === "reviewing" ? t("detail.action_reviewing", "Reviewing")
    : task.status === "awaitingAcceptance" ? t("detail.action_awaiting", "Awaiting your acceptance")
    : task.status === "paused" ? t("detail.action_paused", "Paused")
    : task.status === "drafting" ? t("detail.action_drafting", "Drafting / planning")
    : task.status === "done" ? t("detail.action_done", "Completed")
    : t("detail.action_abandoned", "Abandoned");

  return (
    <aside className={styles.rail}>
      <div className={styles.ring_wrap}>
        <div
          className={styles.ring}
          style={{ background: `conic-gradient(${statusColor(task.status)} ${pct}%, var(--color-bg-secondary) ${pct}%)` }}
        >
          <span className={styles.ring_inner}>{summary.done}/{summary.total}</span>
        </div>
        <div className={styles.ring_label}>{t("detail.metric_progress", "P-item progress")}</div>
      </div>

      <Metric label={t("detail.metric_time", "Elapsed")} value={elapsed != null ? fmtDuration(elapsed) : "—"} />
      <Metric
        label={t("detail.metric_tokens", "Output tokens")}
        value={outTok != null ? fmtTokens(outTok) : "—"}
        sub={inTok != null ? t("detail.metric_in", "in {{n}}", { n: fmtTokens(inTok) }) : undefined}
      />
      <Metric label={t("detail.metric_cost", "Cost")} value={cost != null ? `$${cost.toFixed(2)}` : "—"} />

      {task.model && (
        <Metric label={t("detail.metric_model", "Planner model")} value={task.model.replace(/^claude-/, "").replace(/-\d{8}$/, "")} />
      )}

      {summary.failed > 0 && (
        <Metric label={t("detail.metric_failed", "Failed P-items")} value={String(summary.failed)} danger />
      )}

      <div className={styles.metric}>
        <div className={styles.metric_label}>{t("detail.metric_action", "Current")}</div>
        <div className={styles.metric_action} style={{ color: statusColor(task.status) }}>
          ● {currentAction}
        </div>
      </div>
    </aside>
  );
}

function Metric({ label, value, sub, danger }: { label: string; value: string; sub?: string; danger?: boolean }) {
  return (
    <div className={styles.metric}>
      <div className={styles.metric_label}>{label}</div>
      <div className={`${styles.metric_value} ${danger ? styles.metric_danger : ""}`}>
        {value}
        {sub && <small className={styles.metric_sub}> {sub}</small>}
      </div>
    </div>
  );
}
