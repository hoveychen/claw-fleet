// Resource queue: the desktop machine's named resources (`fleet resource`,
// `fleet__job need=[…]`), who holds each one and who is queued for it, read
// over the relay (`resource_list`). The phone can reorder the queue, cancel a
// queued request (`resource_move` / `resource_cancel`) and release a session
// lease on its session's behalf (`resource_lease_release`); kicking a running
// command stays desktop-only. Below the cards, the queue's event log
// (`resource_history`).
//
// Fullscreen overlay from "More", same z-level as AttentionView; the
// HistoryLayer wrapping it in App owns the back gesture.

import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { ChevronDown, ChevronsUp, ChevronUp, Layers, LockOpen, RefreshCw, WifiOff, X } from "lucide-react";

import type { MoveTarget, QueuedRequest, ResourceRequest, ResourceSnapshot } from "../generated/types";
import { t } from "../i18n";
import { useConfirm } from "../confirmDialog";
import {
  commandLabel,
  fmtDuration,
  moveTargets,
  resourceCards,
  unitsOf,
  sessionHolds,
  waitLines,
  workspaceName,
} from "../resources";
import type { FleetTransport } from "../transport";
import { useDelayedFlag } from "../useDelayedFlag";
import { AppHeader } from "./AppHeader";
import { EmptyState } from "./EmptyState";
import { HeaderAction } from "./HeaderAction";
import { ResourceHistorySection } from "./ResourceHistory";
import { SkeletonList, Spinner } from "./loading";
import styles from "./ResourcesView.module.css";

const POLL_MS = 2000;

interface Props {
  client: FleetTransport | null;
  onBack: () => void;
}

function errText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

export function ResourcesView({ client, onBack }: Props) {
  const confirm = useConfirm();
  const [snap, setSnap] = useState<ResourceSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  // Request id whose move/cancel is in flight — its buttons go busy and every
  // other row's stay usable.
  const [busyId, setBusyId] = useState<string | null>(null);
  const inFlight = useRef(false);

  const refresh = useCallback(
    async (manual = false) => {
      if (!client || inFlight.current) return;
      inFlight.current = true;
      if (manual) setRefreshing(true);
      try {
        setSnap(await client.request<ResourceSnapshot>("resource_list"));
        setError(null);
      } catch (e) {
        setError(errText(e));
      } finally {
        inFlight.current = false;
        if (manual) setRefreshing(false);
      }
    },
    [client],
  );
  const showRefreshing = useDelayedFlag(refreshing);

  // Poll while the page is open and the app is in the foreground.
  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => {
      if (document.visibilityState === "visible") void refresh();
    }, POLL_MS);
    return () => window.clearInterval(timer);
  }, [refresh]);

  const act = useCallback(
    async (
      id: string,
      method: "resource_move" | "resource_cancel" | "resource_lease_release",
      params: Record<string, unknown>,
    ) => {
      if (!client) return;
      setBusyId(id);
      setActionError(null);
      try {
        setSnap(await client.request<ResourceSnapshot>(method, { id, ...params }));
      } catch (e) {
        // Typically the request started running or was cancelled elsewhere
        // between two polls; the refresh below shows where it went.
        setActionError(errText(e));
        void refresh();
      } finally {
        setBusyId(null);
      }
    },
    [client, refresh],
  );

  const move = (id: string, target: MoveTarget) => void act(id, "resource_move", { target });
  const cancel = async (q: QueuedRequest) => {
    const ok = await confirm(t("取消排队中的「{0}」？它不会再运行。", commandLabel(q.request, 60)));
    if (ok) await act(q.request.id, "resource_cancel", {});
  };
  const releaseLease = async (r: ResourceRequest) => {
    const ok = await confirm(
      t(
        "替会话 {0} 放掉「{1}」？资源交给下一个排队的请求；会话本身不会被打断，但它不再持有这些资源。",
        (r.sessionId ?? "").slice(0, 8),
        commandLabel(r, 60),
      ),
    );
    if (ok) await act(r.id, "resource_lease_release", {});
  };

  const cards = snap ? resourceCards(snap) : [];

  return (
    <div className={styles.page}>
      <AppHeader
        onBack={onBack}
        title={t("资源与排队")}
        titleAfter={snap && snap.queue.length > 0 && <span className={styles.count}>{snap.queue.length}</span>}
        actions={
          <HeaderAction
            icon={<RefreshCw size={17} />}
            label={t("刷新")}
            onClick={() => void refresh(true)}
            busy={showRefreshing}
            disabled={refreshing}
          />
        }
      />

      <div className={styles.view}>
        {error && <div className={styles.hint}>{t("加载失败：{0}", error)}</div>}
        {actionError && (
          <div className={styles.actionError} role="alert">
            <span>{t("操作失败：{0}", actionError)}</span>
            <button className={styles.dismiss} onClick={() => setActionError(null)} aria-label={t("关闭")}>
              <X size={14} />
            </button>
          </div>
        )}
        {!error && snap === null &&
          (client ? <SkeletonList rows={3} /> : <EmptyState icon={WifiOff} title={t("桌面端离线")} />)}
        {snap && cards.length === 0 && (
          <EmptyState
            icon={Layers}
            title={t("还没有资源")}
            description={t("命令第一次用 --need 声明一个资源时会自动创建它，容量为 1。")}
          />
        )}

        {snap &&
          cards.map((card) => (
            <section key={card.name} className={styles.card}>
              <div className={styles.head}>
                <span className={styles.name}>{card.name}</span>
                <span className={styles.usage} data-full={card.inUse >= card.capacity || undefined}>
                  {t("占用 {0}/{1}", card.inUse, card.capacity)}
                </span>
                {card.waiters.length > 0 && (
                  <span className={styles.meta}>{t("{0} 个在排队", card.waiters.length)}</span>
                )}
              </div>

              {card.holders.length > 0 && (
                <div className={styles.block}>
                  <div className={styles.blockLabel}>{t("持有中")}</div>
                  {card.holders.map((r) => (
                    <HolderRow
                      key={r.id}
                      r={r}
                      resource={card.name}
                      nowMs={snap.nowMs}
                      busy={busyId === r.id}
                      locked={busyId !== null}
                      onRelease={() => void releaseLease(r)}
                    />
                  ))}
                </div>
              )}

              {card.waiters.length > 0 && (
                <div className={styles.block}>
                  <div className={styles.blockLabel}>{t("排队中")}</div>
                  {card.waiters.map((q, i) => {
                    const targets = moveTargets(card.waiters, i);
                    const busy = busyId === q.request.id;
                    const locked = busyId !== null;
                    return (
                      <div key={q.request.id} className={styles.row} data-waiter>
                        <div className={styles.rowMain}>
                          <div className={styles.line}>
                            <span className={styles.position}>#{q.position}</span>
                            <span className={styles.command}>{commandLabel(q.request)}</span>
                          </div>
                          <RequestMeta r={q.request} resource={card.name} />
                          <div className={styles.meta}>
                            {t("已等 {0}", fmtDuration(snap.nowMs - q.request.enqueuedMs))}
                            {q.request.priority !== "normal" && (
                              <span className={styles.priority} data-priority={q.request.priority}>
                                {q.request.priority === "high" ? t("高优先级") : t("低优先级")}
                              </span>
                            )}
                            {(q.request.skips ?? 0) > 0 && (
                              <span className={styles.priority}>{t("已被插空 {0} 次", q.request.skips ?? 0)}</span>
                            )}
                            {sessionHolds(q.request, snap.holders) && (
                              <span className={styles.priority}>{t("已持有 {0}", sessionHolds(q.request, snap.holders))}</span>
                            )}
                          </div>
                          {waitLines(q, snap).map((line) => (
                            <div key={line} className={styles.wait}>
                              {line}
                            </div>
                          ))}
                        </div>
                        <div className={styles.controls}>
                          <IconButton
                            label={t("移到队首")}
                            disabled={locked || !targets.top}
                            onClick={() => targets.top && move(q.request.id, targets.top)}
                          >
                            <ChevronsUp size={16} />
                          </IconButton>
                          <IconButton
                            label={t("上移")}
                            disabled={locked || !targets.up}
                            onClick={() => targets.up && move(q.request.id, targets.up)}
                          >
                            <ChevronUp size={16} />
                          </IconButton>
                          <IconButton
                            label={t("下移")}
                            disabled={locked || !targets.down}
                            onClick={() => targets.down && move(q.request.id, targets.down)}
                          >
                            <ChevronDown size={16} />
                          </IconButton>
                          <IconButton
                            label={t("取消排队")}
                            danger
                            disabled={locked}
                            onClick={() => void cancel(q)}
                          >
                            {busy ? <Spinner size={14} /> : <X size={16} />}
                          </IconButton>
                        </div>
                      </div>
                    );
                  })}
                </div>
              )}

              {card.holders.length === 0 && card.waiters.length === 0 && (
                <div className={styles.idle}>{t("空闲")}</div>
              )}
            </section>
          ))}
        {snap && <ResourceHistorySection client={client} resources={cards.map((c) => c.name)} />}
      </div>
    </div>
  );
}

function RequestMeta({ r, resource }: { r: ResourceRequest; resource: string }) {
  const ws = workspaceName(r.workspace);
  const units = unitsOf(r, resource);
  const others = Object.keys(r.needs).filter((n) => n !== resource);
  if (!ws && units <= 1 && others.length === 0) return null;
  return (
    <div className={styles.meta}>
      {[
        ws,
        units > 1 ? t("占 {0} 份", units) : null,
        others.length > 0 ? t("还要 {0}", others.join(", ")) : null,
      ]
        .filter(Boolean)
        .join(" · ")}
    </div>
  );
}

function HolderRow({
  r,
  resource,
  nowMs,
  busy,
  locked,
  onRelease,
}: {
  r: ResourceRequest;
  resource: string;
  nowMs: number;
  busy: boolean;
  locked: boolean;
  onRelease: () => void;
}) {
  const lease = r.kind === "lease";
  return (
    <div className={styles.row}>
      <div className={styles.rowMain}>
        <div className={styles.line}>
          <span className={styles.dot} aria-hidden />
          <span className={styles.command}>{commandLabel(r)}</span>
        </div>
        <RequestMeta r={r} resource={resource} />
        <div className={styles.meta}>
          {t("已占用 {0}", fmtDuration(nowMs - (r.grantedMs ?? r.enqueuedMs)))}
          {lease && r.sessionId && <span>{t("会话 {0}", r.sessionId.slice(0, 8))}</span>}
          {lease && r.idleSinceMs != null && (
            <span className={styles.idleTag}>
              {t("会话已无活动 {0}，即将自动释放", fmtDuration(nowMs - r.idleSinceMs))}
            </span>
          )}
        </div>
      </div>
      {lease && (
        <div className={styles.controls}>
          <IconButton label={t("替会话放掉租约")} danger disabled={locked} onClick={onRelease}>
            {busy ? <Spinner size={14} /> : <LockOpen size={16} />}
          </IconButton>
        </div>
      )}
    </div>
  );
}

function IconButton({
  label,
  disabled,
  danger,
  onClick,
  children,
}: {
  label: string;
  disabled?: boolean;
  danger?: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      className={styles.iconBtn}
      data-danger={danger || undefined}
      aria-label={label}
      title={label}
      disabled={disabled}
      onClick={onClick}
    >
      {children}
    </button>
  );
}
