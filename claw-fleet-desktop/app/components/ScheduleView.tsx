import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { CalendarClock, Repeat, Trash2, RefreshCw, Plus } from "lucide-react";
import { EmptyState } from "./EmptyState";
import { PageShell } from "./PageShell";
import { useUIStore } from "../store";
import styles from "./ScheduleView.module.css";

// ── Create shortcut helpers ──────────────────────────────────────────────────
// The "新建" button is a shortcut, not a form: the user only picks a time, then
// we open a NEW session seeded with a scheduling-assistant template so the agent
// (not the user) authors the schedule content via `fleet schedule create`.

/** A Date → `YYYY-MM-DDTHH:MM` local string, the shape a datetime-local input
 *  wants and the exact form `schedule::parse_at` accepts. */
function toLocalInput(d: Date): string {
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(d.getHours())}:${p(d.getMinutes())}`;
}

/** The seed prompt: tells the agent to confirm the task then schedule it. */
function scheduleTemplate(fireLocal: string): string {
  const human = fireLocal.replace("T", " ");
  return [
    `请协助我安排一个定时任务,预定在 ${human} 触发。`,
    ``,
    `先跟我确认清楚要做的事,理解需求后用下面的命令把它排程(需要时可加 --model / --effort 指定模型和推理档位):`,
    ``,
    `    fleet schedule create --at "${fireLocal}" --prompt "<把要做的事写清楚>"`,
    ``,
    `事情是:`,
  ].join("\n");
}

// ── Types (mirror the Rust serde camelCase records) ──────────────────────────

interface LoopRecord {
  id: string;
  workspacePath: string;
  prompt: string;
  intervalSecs: number;
  nextFireAt: number;
  iterationsDone: number;
  maxIterations?: number;
  model?: string;
  agentSource?: string;
  lastSessionId?: string;
}

interface ScheduleRecord {
  id: string;
  workspacePath: string;
  prompt: string;
  fireAt: number;
  status: "pending" | "fired";
  firedAt?: number;
  model?: string;
  agentSource?: string;
  firedSessionId?: string;
}

// Unified "future task" row — a loop or a one-shot schedule.
type FutureTask =
  | { kind: "loop"; id: string; rec: LoopRecord }
  | { kind: "schedule"; id: string; rec: ScheduleRecord };

// ── Helpers ──────────────────────────────────────────────────────────────────

function baseName(path: string): string {
  const parts = path.replace(/\/+$/, "").split("/");
  return parts[parts.length - 1] || path;
}

function fmtInterval(secs: number): string {
  if (secs % 86400 === 0) return `${secs / 86400}d`;
  if (secs % 3600 === 0) return `${secs / 3600}h`;
  if (secs % 60 === 0) return `${secs / 60}m`;
  return `${secs}s`;
}

function fmtUntil(ms: number): string {
  const diff = ms - Date.now();
  if (diff <= 0) return "due";
  const s = Math.floor(diff / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h${m % 60 ? `${m % 60}m` : ""}`;
  const d = Math.floor(h / 24);
  return `${d}d${h % 24 ? `${h % 24}h` : ""}`;
}

function fmtAbsolute(ms: number): string {
  try {
    return new Date(ms).toLocaleString(undefined, {
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    });
  } catch {
    return String(ms);
  }
}

function fmtAgo(ms: number): string {
  const s = Math.floor((Date.now() - ms) / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h`;
  return `${Math.floor(h / 24)}d`;
}

function nextFireOf(task: FutureTask): number {
  return task.kind === "loop" ? task.rec.nextFireAt : task.rec.fireAt;
}

// ── Component ────────────────────────────────────────────────────────────────

export function ScheduleView() {
  const { t } = useTranslation();
  const [loops, setLoops] = useState<LoopRecord[]>([]);
  const [schedules, setSchedules] = useState<ScheduleRecord[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [cancelling, setCancelling] = useState<Set<string>>(new Set());

  const load = useCallback(async () => {
    try {
      const [ls, ss] = await Promise.all([
        invoke<LoopRecord[]>("list_loops"),
        invoke<ScheduleRecord[]>("list_schedules"),
      ]);
      setLoops(ls);
      setSchedules(ss);
    } catch {
      /* leave last-known state */
    } finally {
      setLoaded(true);
    }
  }, []);

  useEffect(() => {
    load();
    // Loops/schedules change out-of-process (CLI, detached timers, reconcile),
    // so there's no Tauri event to listen for — poll while the view is open.
    const id = setInterval(load, 8000);
    return () => clearInterval(id);
  }, [load]);

  const cancel = useCallback(
    async (task: FutureTask) => {
      setCancelling((prev) => new Set(prev).add(task.id));
      try {
        await invoke(task.kind === "loop" ? "cancel_loop" : "cancel_schedule", {
          id: task.id,
        });
        await load();
      } finally {
        setCancelling((prev) => {
          const next = new Set(prev);
          next.delete(task.id);
          return next;
        });
      }
    },
    [load],
  );

  // Unified list, soonest-due first. Pending items lead; fired history trails.
  const tasks = useMemo<FutureTask[]>(() => {
    const merged: FutureTask[] = [
      ...loops.map((rec) => ({ kind: "loop" as const, id: rec.id, rec })),
      ...schedules.map((rec) => ({ kind: "schedule" as const, id: rec.id, rec })),
    ];
    merged.sort((a, b) => {
      const aFired = a.kind === "schedule" && a.rec.status === "fired";
      const bFired = b.kind === "schedule" && b.rec.status === "fired";
      if (aFired !== bFired) return aFired ? 1 : -1; // pending first
      return nextFireOf(a) - nextFireOf(b);
    });
    return merged;
  }, [loops, schedules]);

  const pendingCount = useMemo(
    () =>
      tasks.filter((x) => !(x.kind === "schedule" && x.rec.status === "fired"))
        .length,
    [tasks],
  );

  // "新建" shortcut: pick a time → open a new session seeded with the template.
  const requestNewSession = useUIStore((s) => s.requestNewSession);
  const [creating, setCreating] = useState(false);
  const [fireLocal, setFireLocal] = useState("");

  const openCreate = useCallback(() => {
    setFireLocal(toLocalInput(new Date(Date.now() + 60 * 60 * 1000))); // default +1h
    setCreating(true);
  }, []);

  const confirmCreate = useCallback(() => {
    if (!fireLocal) return;
    requestNewSession({ prompt: scheduleTemplate(fireLocal) });
    setCreating(false);
  }, [fireLocal, requestNewSession]);

  return (
    <PageShell
      view="schedule"
      title={t("schedule.title", "计划任务")}
      count={loaded && pendingCount > 0 ? pendingCount : null}
      bannerCenter={
        <div className={styles.banner_actions}>
          <button className={styles.create_btn} onClick={openCreate} title={t("schedule.create", "新建计划")}>
            <Plus size={14} strokeWidth={2.4} />
            {t("schedule.create", "新建")}
          </button>
          <button className={styles.refresh} onClick={load} title={t("schedule.refresh", "刷新")}>
            <RefreshCw size={14} strokeWidth={2} />
          </button>
        </div>
      }
    >
      {creating && (
        <CreateModal
          fireLocal={fireLocal}
          setFireLocal={setFireLocal}
          onConfirm={confirmCreate}
          onCancel={() => setCreating(false)}
        />
      )}
      <div className={styles.list}>
        {loaded && tasks.length === 0 && (
          <EmptyState
            icon={<CalendarClock size={28} strokeWidth={1.5} />}
            title={t("schedule.empty_title", "还没有计划任务")}
            subtitle={t(
              "schedule.empty_sub",
              "用 `fleet schedule create --in 5d` 或 `fleet loop create` 安排一个未来任务",
            )}
          />
        )}
        {tasks.map((task) => (
          <TaskRow
            key={`${task.kind}:${task.id}`}
            task={task}
            cancelling={cancelling.has(task.id)}
            onCancel={() => cancel(task)}
          />
        ))}
      </div>
    </PageShell>
  );
}

// ── Create modal ─────────────────────────────────────────────────────────────
// Just a time picker. Confirming hands off to a new session; the agent writes
// the actual schedule. Deliberately no prompt/model fields here — that content
// is the agent's job, per the "教育用户内容应由 agent 创建" design.

function CreateModal({
  fireLocal,
  setFireLocal,
  onConfirm,
  onCancel,
}: {
  fireLocal: string;
  setFireLocal: (v: string) => void;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const { t } = useTranslation();
  const preset = (fn: (d: Date) => Date) => setFireLocal(toLocalInput(fn(new Date())));
  const presets: Array<{ label: string; fn: (d: Date) => Date }> = [
    { label: t("schedule.preset_1h", "+1 小时"), fn: (d) => new Date(d.getTime() + 3600_000) },
    {
      label: t("schedule.preset_tonight", "今晚 20:00"),
      fn: (d) => {
        const x = new Date(d);
        x.setHours(20, 0, 0, 0);
        if (x.getTime() <= d.getTime()) x.setDate(x.getDate() + 1);
        return x;
      },
    },
    {
      label: t("schedule.preset_tomorrow", "明早 09:00"),
      fn: (d) => {
        const x = new Date(d);
        x.setDate(x.getDate() + 1);
        x.setHours(9, 0, 0, 0);
        return x;
      },
    },
    { label: t("schedule.preset_1d", "+1 天"), fn: (d) => new Date(d.getTime() + 86_400_000) },
  ];

  return (
    <div className={styles.modal_overlay} onClick={onCancel}>
      <div className={styles.modal} onClick={(e) => e.stopPropagation()}>
        <div className={styles.modal_title}>{t("schedule.create_title", "新建计划任务")}</div>
        <div className={styles.modal_hint}>
          {t(
            "schedule.create_hint",
            "选个触发时间,然后去新会话把事情讲给 agent —— 由它替你把计划创建好。",
          )}
        </div>
        <label className={styles.field_label}>{t("schedule.fire_at", "触发时间")}</label>
        <input
          className={styles.time_input}
          type="datetime-local"
          value={fireLocal}
          onChange={(e) => setFireLocal(e.target.value)}
        />
        <div className={styles.presets}>
          {presets.map((p) => (
            <button key={p.label} className={styles.preset} onClick={() => preset(p.fn)}>
              {p.label}
            </button>
          ))}
        </div>
        <div className={styles.modal_actions}>
          <button className={styles.btn_ghost} onClick={onCancel}>
            {t("schedule.cancel", "取消")}
          </button>
          <button className={styles.btn_primary} onClick={onConfirm} disabled={!fireLocal}>
            {t("schedule.go_new_session", "去新会话")}
          </button>
        </div>
      </div>
    </div>
  );
}

// ── Row ──────────────────────────────────────────────────────────────────────

function TaskRow({
  task,
  cancelling,
  onCancel,
}: {
  task: FutureTask;
  cancelling: boolean;
  onCancel: () => void;
}) {
  const { t } = useTranslation();
  const rec = task.rec;
  const isLoop = task.kind === "loop";
  const fired = task.kind === "schedule" && task.rec.status === "fired";

  // Prompt can be long; clamp to 2 lines and let the user click to expand.
  const [expanded, setExpanded] = useState(false);
  const promptRef = useRef<HTMLDivElement>(null);
  const [overflowing, setOverflowing] = useState(false);
  useEffect(() => {
    const el = promptRef.current;
    if (!el) return;
    // When clamped, the box's real content is taller than its rendered height.
    setOverflowing(el.scrollHeight > el.clientHeight + 1);
  }, [rec.prompt, expanded]);
  const expandable = overflowing || expanded;

  // Timing summary line.
  let timing: string;
  if (isLoop) {
    const l = task.rec as LoopRecord;
    timing = `${t("schedule.every", "每")} ${fmtInterval(l.intervalSecs)} · ${t(
      "schedule.next",
      "下次",
    )} ${fmtUntil(l.nextFireAt)}`;
  } else if (fired) {
    const s = task.rec as ScheduleRecord;
    timing = `${t("schedule.fired", "已触发")} ${s.firedAt ? `${fmtAgo(s.firedAt)} ${t("schedule.ago", "前")}` : ""}`;
  } else {
    const s = task.rec as ScheduleRecord;
    timing = `${t("schedule.fires", "触发")} ${fmtAbsolute(s.fireAt)} · ${t("schedule.in", "还有")} ${fmtUntil(s.fireAt)}`;
  }

  return (
    <div className={`${styles.row} ${fired ? styles.row_fired : ""}`}>
      <span className={`${styles.badge} ${isLoop ? styles.badge_loop : styles.badge_once}`}>
        {isLoop ? <Repeat size={12} strokeWidth={2.2} /> : <CalendarClock size={12} strokeWidth={2.2} />}
        {isLoop ? t("schedule.badge_loop", "循环") : t("schedule.badge_once", "单次")}
      </span>
      <div className={styles.body}>
        <div
          ref={promptRef}
          className={`${styles.prompt} ${expanded ? styles.prompt_expanded : ""} ${expandable ? styles.prompt_clickable : ""}`}
          onClick={expandable ? () => setExpanded((v) => !v) : undefined}
          role={expandable ? "button" : undefined}
          tabIndex={expandable ? 0 : undefined}
          onKeyDown={
            expandable
              ? (e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    setExpanded((v) => !v);
                  }
                }
              : undefined
          }
          title={expandable ? (expanded ? t("schedule.collapse", "点击收起") : t("schedule.expand", "点击展开全文")) : undefined}
        >
          {rec.prompt}
        </div>
        <div className={styles.meta}>
          <span className={fired ? styles.status_fired : styles.status_pending}>
            {fired ? t("schedule.status_fired", "已触发") : t("schedule.status_pending", "待触发")}
          </span>
          <span className={styles.dot}>·</span>
          <span>{timing}</span>
          <span className={styles.dot}>·</span>
          <span title={rec.workspacePath}>{baseName(rec.workspacePath)}</span>
          {rec.agentSource === "codex" && (
            <>
              <span className={styles.dot}>·</span>
              <span>Codex</span>
            </>
          )}
          <span className={styles.dot}>·</span>
          <span className={styles.id}>{rec.id}</span>
        </div>
      </div>
      <button
        className={styles.cancel}
        disabled={cancelling}
        onClick={onCancel}
        title={t("schedule.cancel", "取消")}
      >
        <Trash2 size={13} strokeWidth={2} />
        {cancelling ? t("schedule.cancelling", "取消中") : t("schedule.cancel", "取消")}
      </button>
    </div>
  );
}
