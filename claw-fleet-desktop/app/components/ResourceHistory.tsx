import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { invoke } from "@tauri-apps/api/core";
import type { HistoryQuery, ResourceEvent, ResourceEventKind, ResourceHistory } from "../generated/types";
import { Spinner } from "./loading";
import styles from "./ResourceHistory.module.css";

/** Newest page size; "load more" pages back by the same amount. */
const PAGE = 50;
const POLL_MS = 3000;

/** A stable key for one event (several events can share a millisecond). */
export function eventKey(e: ResourceEvent): string {
  return `${e.tsMs}:${e.kind}:${e.requestId ?? e.resource ?? ""}`;
}

/** Fold a freshly polled newest page into what is shown: the new page replaces
 *  the head, older pages the user loaded stay below it, no event twice. */
export function mergeHistory(head: ResourceEvent[], shown: ResourceEvent[]): ResourceEvent[] {
  const seen = new Set(head.map(eventKey));
  const oldest = head.length ? head[head.length - 1].tsMs : Infinity;
  return [...head, ...shown.filter((e) => e.tsMs <= oldest && !seen.has(eventKey(e)))];
}

/** The resource queue's event log (`resource_queue::history`), newest first,
 *  filterable by resource. */
export function ResourceHistorySection({ resources }: { resources: string[] }) {
  const { t } = useTranslation();
  const [filter, setFilter] = useState<string>("");
  const [events, setEvents] = useState<ResourceEvent[] | null>(null);
  const [hasMore, setHasMore] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // A response for a filter the user has since changed is dropped.
  const filterRef = useRef(filter);
  filterRef.current = filter;
  const pagedBack = useRef(false);

  const fetchPage = useCallback(
    (beforeMs?: number) => {
      const query: HistoryQuery = { limit: PAGE };
      if (filter) query.resource = filter;
      if (beforeMs !== undefined) query.beforeMs = beforeMs;
      return invoke<ResourceHistory>("resource_history", { query });
    },
    [filter],
  );

  useEffect(() => {
    let alive = true;
    let inFlight = false;
    setEvents(null);
    pagedBack.current = false;
    const mine = filter;
    const tick = async () => {
      if (inFlight || document.hidden) return;
      inFlight = true;
      try {
        const page = await fetchPage();
        if (!alive || filterRef.current !== mine) return;
        setEvents((shown) => mergeHistory(page.events, shown ?? []));
        if (!pagedBack.current) setHasMore(page.hasMore);
        setError(null);
      } catch (e) {
        if (alive) setError(String(e));
      } finally {
        inFlight = false;
      }
    };
    void tick();
    const timer = window.setInterval(() => void tick(), POLL_MS);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [fetchPage, filter]);

  const loadMore = async () => {
    if (!events?.length) return;
    setLoadingMore(true);
    const mine = filter;
    try {
      const page = await fetchPage(events[events.length - 1].tsMs);
      if (filterRef.current !== mine) return;
      pagedBack.current = true;
      setEvents((shown) => {
        const have = new Set((shown ?? []).map(eventKey));
        return [...(shown ?? []), ...page.events.filter((e) => !have.has(eventKey(e)))];
      });
      setHasMore(page.hasMore);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoadingMore(false);
    }
  };

  return (
    <section className={styles.history} aria-label={t("resources.history", "历史")}>
      <header className={styles.head}>
        <span className={styles.title}>{t("resources.history", "历史")}</span>
        <span className={styles.spacer} />
        <select
          className={styles.filter}
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          aria-label={t("resources.history_filter", "按资源筛选")}
        >
          <option value="">{t("resources.history_all", "全部资源")}</option>
          {resources.map((r) => (
            <option key={r} value={r}>
              {r}
            </option>
          ))}
        </select>
      </header>
      {error && <div className={styles.error}>{error}</div>}
      {events === null ? (
        <div className={styles.none}>
          <Spinner size={10} />
        </div>
      ) : events.length === 0 ? (
        <div className={styles.none}>{t("resources.history_empty", "还没有记录")}</div>
      ) : (
        <ul className={styles.rows}>
          {events.map((e) => (
            <EventRow key={eventKey(e)} e={e} />
          ))}
        </ul>
      )}
      {hasMore && events && events.length > 0 && (
        <button type="button" className={styles.more} disabled={loadingMore} onClick={() => void loadMore()}>
          {loadingMore ? <Spinner size={10} /> : t("resources.history_more", "加载更早的")}
        </button>
      )}
    </section>
  );
}

const TONE: Record<ResourceEventKind, string> = {
  enqueued: "neutral",
  granted: "accent",
  released: "ok",
  kicked: "bad",
  cancelled: "warn",
  timed_out: "warn",
  aborted: "warn",
  reaped: "bad",
  capacity_set: "neutral",
  resource_removed: "neutral",
};

export function kindLabel(kind: ResourceEventKind, t: TFunction): string {
  switch (kind) {
    case "enqueued":
      return t("resources.ev_enqueued", "入队");
    case "granted":
      return t("resources.ev_granted", "拿到");
    case "released":
      return t("resources.ev_released", "释放");
    case "kicked":
      return t("resources.ev_kicked", "被踢");
    case "cancelled":
      return t("resources.ev_cancelled", "已取消");
    case "timed_out":
      return t("resources.ev_timed_out", "等待超时");
    case "aborted":
      return t("resources.ev_aborted", "放弃排队");
    case "reaped":
      return t("resources.ev_reaped", "已回收");
    case "capacity_set":
      return t("resources.ev_capacity_set", "容量变更");
    case "resource_removed":
      return t("resources.ev_resource_removed", "资源删除");
  }
}

function EventRow({ e }: { e: ResourceEvent }) {
  const { t } = useTranslation();
  const needs = Object.entries(e.needs ?? {})
    .map(([n, u]) => (u === 1 ? n : `${n}:${u}`))
    .join(", ");
  const cmd = (e.command ?? "").split(/\s+/).filter(Boolean).join(" ");
  return (
    <li className={styles.row}>
      <span className={styles.when} title={new Date(e.tsMs).toLocaleString()}>
        {fmtWhen(e.tsMs)}
      </span>
      <span
        className={styles.kind}
        data-tone={TONE[e.kind] ?? "neutral"}
        title={e.kind === "reaped" ? t("resources.ev_reaped_hint", "持有者进程已消失，资源被自动收回") : undefined}
      >
        {kindLabel(e.kind, t)}
      </span>
      <div className={styles.main}>
        {e.resource ? (
          <span className={styles.what}>
            <code>{e.resource}</code>
            {e.capacity != null &&
              (e.prevCapacity != null
                ? ` ${t("resources.ev_capacity_from_to", "容量 {{from}} → {{to}}", { from: e.prevCapacity, to: e.capacity })}`
                : ` ${t("resources.ev_capacity_to", "容量 {{to}}", { to: e.capacity })}`)}
          </span>
        ) : (
          <code className={styles.command} title={cmd || undefined}>
            {cmd || (e.requestId ?? "")}
          </code>
        )}
        <span className={styles.meta}>
          {needs && <span className={styles.needs}>{needs}</span>}
          {e.waitedMs != null && e.kind !== "enqueued" && (
            <span>{t("resources.ev_waited", "等了 {{d}}", { d: fmtDur(e.waitedMs, t) })}</span>
          )}
          {e.heldMs != null && <span>{t("resources.ev_held", "持有 {{d}}", { d: fmtDur(e.heldMs, t) })}</span>}
          {e.exitCode != null && (
            <span className={e.exitCode === 0 ? undefined : styles.bad_exit}>
              {t("resources.ev_exit", "退出码 {{code}}", { code: e.exitCode })}
            </span>
          )}
          {e.sessionId && <span className={styles.sid}>{e.sessionId.slice(0, 8)}</span>}
          {e.pid != null && <span className={styles.pid}>pid {e.pid}</span>}
        </span>
      </div>
    </li>
  );
}

function fmtWhen(ms: number): string {
  const d = new Date(ms);
  const pad = (n: number) => String(n).padStart(2, "0");
  const time = `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
  const today = new Date();
  if (d.toDateString() === today.toDateString()) return time;
  return `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${time}`;
}

function fmtDur(ms: number, t: TFunction): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  if (h > 0) return t("resources.dur_h", "{{h}}时{{m}}分", { h, m });
  if (m > 0) return t("resources.dur_m", "{{m}}分{{s}}秒", { m, s: sec });
  if (s === 0 && ms > 0) return t("resources.dur_ms", "{{ms}}毫秒", { ms });
  return t("resources.dur_s", "{{s}}秒", { s: sec });
}
