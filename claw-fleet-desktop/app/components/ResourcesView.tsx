import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { invoke } from "@tauri-apps/api/core";
import {
  ArrowUpToLine,
  Boxes,
  GripVertical,
  Minus,
  Plus,
  Power,
  Trash2,
  X,
} from "lucide-react";
import type {
  MoveTarget,
  QueuedRequest,
  ResourceRequest,
  ResourceSnapshot,
  ResourceView,
  WaitOn,
} from "../generated/types";
import { useUIStore } from "../store";
import { dropTargetAt, usePointerDrag } from "../hooks/usePointerDrag";
import { PageShell } from "./PageShell";
import { EmptyState } from "./EmptyState";
import { ConfirmDialog } from "./ConfirmDialog";
import { Presence } from "./Presence";
import { SkeletonList, Spinner } from "./loading";
import { ResourceHistorySection } from "./ResourceHistory";
import styles from "./ResourcesView.module.css";

/** How often the table is re-read while the page is open. Holders come and go
 *  on their own (a command exits, a waiter is granted), so this is the only way
 *  the page learns about them. */
const POLL_MS = 1500;

/** The hand-written view of the machine-global resource queue
 *  (`claw_fleet_core::resource_queue`): each resource with its holders and the
 *  requests waiting for it, and the adjustments a person can make — reorder
 *  (drag, or jump to the head), cancel a queued request, kick a holder, change
 *  a capacity, delete an idle resource. */
export function ResourcesView() {
  const { t } = useTranslation();
  const [snap, setSnap] = useState<ResourceSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [kickTarget, setKickTarget] = useState<ResourceRequest | null>(null);

  // Every call gets a sequence number and only a response newer than the last
  // one applied is shown. A poll that left before a write but lands after it
  // would otherwise paint the table from before the write over its result.
  const seq = useRef(0);
  const applied = useRef(0);
  const call = useCallback(
    async (run: () => Promise<ResourceSnapshot>, opts?: { quiet?: boolean }) => {
      const mine = ++seq.current;
      try {
        const next = await run();
        if (mine > applied.current) {
          applied.current = mine;
          setSnap(next);
          if (!opts?.quiet) setError(null);
        }
        return true;
      } catch (e) {
        if (!opts?.quiet || mine > applied.current) setError(errorText(e));
        return false;
      }
    },
    [],
  );

  // Single-flight poll, paused while the window is hidden.
  useEffect(() => {
    let inFlight = false;
    let alive = true;
    const tick = async () => {
      if (inFlight || document.hidden) return;
      inFlight = true;
      await call(() => invoke<ResourceSnapshot>("resource_snapshot"), { quiet: true });
      inFlight = false;
    };
    void tick();
    const timer = window.setInterval(() => alive && void tick(), POLL_MS);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [call]);

  const act = useCallback(
    async (key: string, run: () => Promise<ResourceSnapshot>) => {
      setBusy(key);
      const ok = await call(run);
      setBusy(null);
      return ok;
    },
    [call],
  );

  const byId = useMemo(() => {
    const m = new Map<string, ResourceRequest>();
    for (const h of snap?.holders ?? []) m.set(h.id, h);
    for (const q of snap?.queue ?? []) m.set(q.request.id, q.request);
    return m;
  }, [snap]);
  const queued = useMemo(() => new Map((snap?.queue ?? []).map((q) => [q.request.id, q])), [snap]);

  // ── Drag to reorder ───────────────────────────────────────────────────────
  // One drag hook for the page; the row that was pressed is remembered here.
  const pressed = useRef<string | null>(null);
  const [dragging, setDragging] = useState<string | null>(null);
  const [dropAt, setDropAt] = useState<{ id: string; after: boolean; card: string | null } | null>(null);
  const dropFrom = (over: Element | null, y: number) => {
    const id = dropTargetAt(over, "data-rq-waiter");
    if (!id) return null;
    const row = over?.closest("[data-rq-waiter]");
    const rect = row?.getBoundingClientRect();
    // A multi-resource request has a row on each of its cards; the line goes
    // only on the card under the pointer.
    const card = dropTargetAt(over, "data-rq-card");
    return { id, after: rect ? y > rect.top + rect.height / 2 : false, card };
  };
  const drag = usePointerDrag({
    onStart: () => {
      if (!pressed.current) return false;
      setDragging(pressed.current);
    },
    onMove: (p) => setDropAt(dropFrom(p.over, p.y)),
    onDrop: (p) => {
      const id = dragging ?? pressed.current;
      const at = dropFrom(p.over, p.y);
      setDragging(null);
      setDropAt(null);
      if (!id || !at || !snap) return;
      const target = moveTargetFor(snap.queue, id, at.id, at.after);
      if (target) void act(`move:${id}`, () => invoke<ResourceSnapshot>("resource_move", { id, target }));
    },
    onCancel: () => {
      setDragging(null);
      setDropAt(null);
    },
  });

  const openSession = (sessionId: string) => useUIStore.getState().requestOpenTask(sessionId);

  const now = snap?.nowMs ?? Date.now();
  const resources = snap?.resources ?? [];
  const waitingCount = snap?.queue.length ?? 0;

  return (
    <PageShell
      view="resources"
      title={t("resources.title", "资源")}
      count={snap ? resources.length : null}
      countLoading={!snap}
      bannerCenter={
        snap && (
          <span className={styles.summary}>
            {t("resources.summary", "{{holding}} 个持有 · {{waiting}} 个排队", {
              holding: snap.holders.length,
              waiting: waitingCount,
            })}
          </span>
        )
      }
      afterBody={
        <Presence when={kickTarget !== null}>
          {kickTarget && (
            <ConfirmDialog
              message={t("resources.kick_confirm", "踢掉「{{command}}」？它的命令会被终止（先 SIGTERM，几秒后 SIGKILL），占用的资源交给下一个排队的请求。", {
                command: shortCommand(kickTarget.command, t),
              })}
              busy={busy === `kick:${kickTarget.id}`}
              onCancel={() => setKickTarget(null)}
              onConfirm={async () => {
                const id = kickTarget.id;
                await act(`kick:${id}`, () => invoke<ResourceSnapshot>("resource_kick", { id }));
                setKickTarget(null);
              }}
            />
          )}
        </Presence>
      }
    >
      <div className={styles.page}>
        {error && (
          <div className={styles.error} role="alert">
            <span>{error}</span>
            <button type="button" className={styles.icon_btn} onClick={() => setError(null)} aria-label={t("resources.dismiss", "关闭")}>
              <X size={12} />
            </button>
          </div>
        )}
        {!snap ? (
          <SkeletonList rows={4} />
        ) : resources.length === 0 ? (
          <EmptyState
            icon={<Boxes size={28} strokeWidth={1.5} />}
            title={t("resources.empty_title", "还没有资源")}
            subtitle={t(
              "resources.empty_subtitle",
              "命令第一次用 --need 声明某个资源时它就会出现在这里，例如 fleet job run --need test-db -- <命令>。",
            )}
          />
        ) : (
          <div className={styles.grid}>
            {resources.map((res) => (
              <ResourceCard
                key={res.name}
                res={res}
                now={now}
                byId={byId}
                queued={queued}
                busy={busy}
                dragging={dragging}
                dropAt={dropAt}
                onPressWaiter={(id, e) => {
                  pressed.current = id;
                  drag.onPointerDown(e);
                }}
                didDrag={drag.didDrag}
                onCapacity={(capacity) =>
                  void act(`cap:${res.name}`, () =>
                    invoke<ResourceSnapshot>("resource_set_capacity", { name: res.name, capacity }),
                  )
                }
                onRemove={() =>
                  void act(`rm:${res.name}`, () => invoke<ResourceSnapshot>("resource_remove", { name: res.name }))
                }
                onTop={(id) => {
                  const target: MoveTarget = { index: 1 };
                  void act(`move:${id}`, () => invoke<ResourceSnapshot>("resource_move", { id, target }));
                }}
                onCancelRequest={(id) => void act(`cancel:${id}`, () => invoke<ResourceSnapshot>("resource_cancel", { id }))}
                onKick={(r) => setKickTarget(r)}
                onOpenSession={openSession}
              />
            ))}
          </div>
        )}
        {snap && <ResourceHistorySection resources={resources.map((r) => r.name)} />}
      </div>
    </PageShell>
  );
}

interface CardProps {
  res: ResourceView;
  now: number;
  byId: Map<string, ResourceRequest>;
  queued: Map<string, QueuedRequest>;
  busy: string | null;
  dragging: string | null;
  dropAt: { id: string; after: boolean; card: string | null } | null;
  onPressWaiter: (id: string, e: React.PointerEvent) => void;
  didDrag: () => boolean;
  onCapacity: (capacity: number) => void;
  onRemove: () => void;
  onTop: (id: string) => void;
  onCancelRequest: (id: string) => void;
  onKick: (r: ResourceRequest) => void;
  onOpenSession: (sessionId: string) => void;
}

function ResourceCard({
  res,
  now,
  byId,
  queued,
  busy,
  dragging,
  dropAt,
  onPressWaiter,
  didDrag,
  onCapacity,
  onRemove,
  onTop,
  onCancelRequest,
  onKick,
  onOpenSession,
}: CardProps) {
  const { t } = useTranslation();
  const idle = res.holders.length === 0 && res.waiters.length === 0;
  const full = res.inUse >= res.capacity;
  const capBusy = busy === `cap:${res.name}`;
  return (
    <section className={styles.card} data-rq-card={res.name}>
      <header className={styles.card_head}>
        <span className={styles.name} title={res.name}>
          {res.name}
        </span>
        <span className={`${styles.usage}${full ? ` ${styles.usage_full}` : ""}`}>
          {t("resources.in_use", "{{used}}/{{cap}} 占用", { used: res.inUse, cap: res.capacity })}
        </span>
        <span className={styles.spacer} />
        <span className={styles.stepper} title={t("resources.capacity_hint", "容量：同时能有多少个持有者")}>
          <button
            type="button"
            className={styles.icon_btn}
            disabled={capBusy || res.capacity <= 1}
            onClick={() => onCapacity(res.capacity - 1)}
            aria-label={t("resources.capacity_down", "减少容量")}
          >
            <Minus size={12} />
          </button>
          <span className={styles.cap_value}>{capBusy ? <Spinner size={10} /> : res.capacity}</span>
          <button
            type="button"
            className={styles.icon_btn}
            disabled={capBusy}
            onClick={() => onCapacity(res.capacity + 1)}
            aria-label={t("resources.capacity_up", "增加容量")}
          >
            <Plus size={12} />
          </button>
        </span>
        {idle && (
          <button
            type="button"
            className={`${styles.icon_btn} ${styles.danger}`}
            disabled={busy === `rm:${res.name}`}
            onClick={onRemove}
            title={t("resources.remove", "删除这个空闲资源")}
            aria-label={t("resources.remove", "删除这个空闲资源")}
          >
            <Trash2 size={12} />
          </button>
        )}
      </header>
      <div className={styles.meter} aria-hidden>
        <div
          className={`${styles.meter_fill}${full ? ` ${styles.meter_full}` : ""}`}
          style={{ width: `${Math.min(100, (res.inUse / Math.max(1, res.capacity)) * 100)}%` }}
        />
      </div>

      <div className={styles.section_label}>{t("resources.holders", "持有中")}</div>
      {res.holders.length === 0 ? (
        <div className={styles.none}>{t("resources.no_holders", "空闲")}</div>
      ) : (
        <ul className={styles.rows}>
          {res.holders.map((id) => {
            const r = byId.get(id);
            if (!r) return null;
            const kicking = busy === `kick:${id}`;
            return (
              <li key={id} className={`${styles.row} ${styles.row_holder}`}>
                <div className={styles.row_main}>
                  <RequestLine r={r} onOpenSession={onOpenSession} />
                  <div className={styles.meta}>
                    <span className={styles.time}>
                      {t("resources.held_for", "已持有 {{d}}", { d: fmtDuration(now - (r.grantedMs ?? r.enqueuedMs), t) })}
                    </span>
                    <NeedChips r={r} except={res.name} />
                  </div>
                </div>
                <button
                  type="button"
                  className={`${styles.action} ${styles.danger}`}
                  disabled={kicking}
                  onClick={() => onKick(r)}
                  title={t("resources.kick_hint", "终止这条命令，把资源交给下一个")}
                >
                  {kicking ? <Spinner size={10} /> : <Power size={12} />}
                  {t("resources.kick", "踢掉")}
                </button>
              </li>
            );
          })}
        </ul>
      )}

      <div className={styles.section_label}>
        {t("resources.waiters", "排队")}
        {res.waiters.length > 1 && <span className={styles.drag_hint}>{t("resources.drag_hint", "拖动调整顺序")}</span>}
      </div>
      {res.waiters.length === 0 ? (
        <div className={styles.none}>{t("resources.no_waiters", "没有排队")}</div>
      ) : (
        <ul className={styles.rows}>
          {res.waiters.map((id) => {
            const q = queued.get(id);
            if (!q) return null;
            const r = q.request;
            const indicator = dropAt?.id === id && dropAt.card === res.name && dragging && dragging !== id ? (dropAt.after ? "after" : "before") : undefined;
            return (
              <li
                key={id}
                data-rq-waiter={id}
                data-drop={indicator}
                className={`${styles.row}${dragging === id ? ` ${styles.row_dragging}` : ""}`}
              >
                <span
                  className={styles.grip}
                  onPointerDown={(e) => onPressWaiter(id, e)}
                  onClick={(e) => {
                    if (didDrag()) e.preventDefault();
                  }}
                  title={t("resources.drag_hint", "拖动调整顺序")}
                >
                  <GripVertical size={12} />
                </span>
                <span className={styles.position} title={t("resources.position_hint", "在全机队列中的位置")}>
                  #{q.position}
                </span>
                <div className={styles.row_main}>
                  <RequestLine r={r} onOpenSession={onOpenSession} />
                  <div className={styles.meta}>
                    <span className={styles.time}>
                      {t("resources.waited_for", "已等 {{d}}", { d: fmtDuration(now - r.enqueuedMs, t) })}
                    </span>
                    {r.manual && <span className={styles.tag}>{t("resources.manual", "手动排序")}</span>}
                    {r.priority !== "normal" && (
                      <span className={styles.tag}>
                        {r.priority === "high" ? t("resources.priority_high", "高优先级") : t("resources.priority_low", "低优先级")}
                      </span>
                    )}
                    <NeedChips r={r} except={res.name} />
                  </div>
                  {q.waitOn.map((w) => (
                    <div key={w.resource} className={styles.wait_on}>
                      {describeWait(w, byId, queued, t)}
                    </div>
                  ))}
                </div>
                <div className={styles.row_actions}>
                  <button
                    type="button"
                    className={styles.icon_btn}
                    disabled={q.position === 1 || busy === `move:${id}`}
                    onClick={() => onTop(id)}
                    title={t("resources.to_top", "插到队首")}
                    aria-label={t("resources.to_top", "插到队首")}
                  >
                    <ArrowUpToLine size={12} />
                  </button>
                  <button
                    type="button"
                    className={`${styles.icon_btn} ${styles.danger}`}
                    disabled={busy === `cancel:${id}`}
                    onClick={() => onCancelRequest(id)}
                    title={t("resources.cancel", "撤掉这个排队请求（命令不会运行）")}
                    aria-label={t("resources.cancel", "撤掉这个排队请求（命令不会运行）")}
                  >
                    <X size={12} />
                  </button>
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}

function RequestLine({ r, onOpenSession }: { r: ResourceRequest; onOpenSession: (id: string) => void }) {
  const { t } = useTranslation();
  const ws = r.workspace ? r.workspace.split("/").filter(Boolean).pop() : null;
  return (
    <div className={styles.line}>
      <span className={styles.kind}>
        {r.kind === "job" ? t("resources.kind_job", "job") : t("resources.kind_fg", "前台")}
      </span>
      <code className={styles.command} title={r.command || undefined}>
        {shortCommand(r.command, t)}
      </code>
      {ws && (
        <span className={styles.ws} title={r.workspace ?? undefined}>
          {ws}
        </span>
      )}
      {r.sessionId && (
        <button
          type="button"
          className={styles.session}
          onClick={() => onOpenSession(r.sessionId!)}
          title={t("resources.open_session", "打开会话 {{id}}", { id: r.sessionId })}
        >
          {r.sessionId.slice(0, 8)}
        </button>
      )}
    </div>
  );
}

/** The other resources a request needs besides this card's: a multi-resource
 *  request shows up on each of its cards, and this says so. */
function NeedChips({ r, except }: { r: ResourceRequest; except: string }) {
  const { t } = useTranslation();
  const others = Object.entries(r.needs).filter(([name]) => name !== except);
  if (others.length === 0) return null;
  return (
    <span className={styles.also}>
      {t("resources.also_needs", "同时需要")}{" "}
      {others.map(([name, n]) => (n === 1 ? name : `${name}:${n}`)).join(", ")}
    </span>
  );
}

function describeWait(
  w: WaitOn,
  byId: Map<string, ResourceRequest>,
  queued: Map<string, QueuedRequest>,
  t: TFunction,
): string {
  if (w.reason === "busy") {
    const who = w.by.map((id) => shortCommand(byId.get(id)?.command ?? id, t, 28)).join("、");
    return t("resources.wait_busy", "等 {{res}}：被 {{who}} 占着", { res: w.resource, who: who || "—" });
  }
  const ahead = w.by
    .map((id) => {
      const pos = queued.get(id)?.position;
      return pos ? `#${pos}` : id;
    })
    .join("、");
  return t("resources.wait_reserved", "等 {{res}}：空着，但留给了排在前面的 {{who}}", { res: w.resource, who: ahead });
}

/** Where to move `id` so it lands directly before / after `targetId`, in the
 *  terms `resource_move` takes. `null` when the drop changes nothing. */
export function moveTargetFor(
  queue: QueuedRequest[],
  id: string,
  targetId: string,
  after: boolean,
): MoveTarget | null {
  if (id === targetId) return null;
  const ids = queue.map((q) => q.request.id);
  const at = ids.indexOf(targetId);
  if (at < 0 || !ids.includes(id)) return null;
  if (!after) {
    return ids[at - 1] === id ? null : { before: targetId };
  }
  const next = ids[at + 1];
  if (next === id) return null;
  return next !== undefined ? { before: next } : { index: ids.length };
}

function shortCommand(cmd: string, t: TFunction, max = 72): string {
  const one = cmd.split(/\s+/).filter(Boolean).join(" ");
  if (!one) return t("resources.private_command", "（隐身会话的命令）");
  return one.length > max ? `${one.slice(0, max - 1)}…` : one;
}

function fmtDuration(ms: number, t: TFunction): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  if (h > 0) return t("resources.dur_h", "{{h}}时{{m}}分", { h, m });
  if (m > 0) return t("resources.dur_m", "{{m}}分{{s}}秒", { m, s: sec });
  return t("resources.dur_s", "{{s}}秒", { s: sec });
}

function errorText(e: unknown): string {
  const text = e instanceof Error ? e.message : String(e);
  // The browser build's transport reports `HTTP 409: {"error": "..."}`.
  const m = /^HTTP \d+: (\{.*\})$/s.exec(text);
  if (m) {
    try {
      const body = JSON.parse(m[1]) as { error?: string };
      if (body.error) return body.error;
    } catch {
      /* fall through */
    }
  }
  return text;
}
