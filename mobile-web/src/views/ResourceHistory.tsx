// The resource queue's event log (`resource_history` over the relay): what was
// queued, granted, released, kicked, cancelled… newest first, with a chip row
// to narrow it to one resource and a "load older" button for paging back.

import { useCallback, useEffect, useRef, useState } from "react";

import type { HistoryQuery, ResourceEvent, ResourceHistory } from "../generated/types";
import { t } from "../i18n";
import { eventDetails, eventKey, eventKind, eventTime, eventTitle } from "../resources";
import type { FleetTransport } from "../transport";
import { Spinner } from "./loading";
import styles from "./ResourceHistory.module.css";

const PAGE = 30;
const POLL_MS = 4000;

export function ResourceHistorySection({
  client,
  resources,
}: {
  client: FleetTransport | null;
  resources: string[];
}) {
  const [filter, setFilter] = useState<string | null>(null);
  const [events, setEvents] = useState<ResourceEvent[] | null>(null);
  const [hasMore, setHasMore] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const filterRef = useRef(filter);
  filterRef.current = filter;
  const pagedBack = useRef(false);

  const fetchPage = useCallback(
    (beforeMs?: number) => {
      if (!client) return Promise.reject(new Error("offline"));
      const query: HistoryQuery = { limit: PAGE };
      if (filter) query.resource = filter;
      if (beforeMs !== undefined) query.beforeMs = beforeMs;
      return client.request<ResourceHistory>("resource_history", query as Record<string, unknown>);
    },
    [client, filter],
  );

  useEffect(() => {
    if (!client) return;
    let alive = true;
    let inFlight = false;
    const mine = filter;
    setEvents(null);
    pagedBack.current = false;
    const tick = async () => {
      if (inFlight || document.visibilityState !== "visible") return;
      inFlight = true;
      try {
        const page = await fetchPage();
        if (!alive || filterRef.current !== mine) return;
        setEvents((shown) => {
          // The new newest page replaces the head; pages loaded by hand stay.
          const seen = new Set(page.events.map(eventKey));
          const oldest = page.events.length ? page.events[page.events.length - 1].tsMs : Infinity;
          return [...page.events, ...(shown ?? []).filter((e) => e.tsMs <= oldest && !seen.has(eventKey(e)))];
        });
        if (!pagedBack.current) setHasMore(page.hasMore);
        setError(null);
      } catch (e) {
        if (alive) setError(e instanceof Error ? e.message : String(e));
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
  }, [client, fetchPage, filter]);

  const loadMore = async () => {
    if (!events?.length) return;
    const mine = filter;
    setLoadingMore(true);
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
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoadingMore(false);
    }
  };

  return (
    <section className={styles.card}>
      <div className={styles.head}>
        <span className={styles.title}>{t("历史")}</span>
      </div>
      {resources.length > 1 && (
        <div className={styles.chips}>
          <button className={styles.chip} data-on={filter === null || undefined} onClick={() => setFilter(null)}>
            {t("全部")}
          </button>
          {resources.map((r) => (
            <button key={r} className={styles.chip} data-on={filter === r || undefined} onClick={() => setFilter(r)}>
              {r}
            </button>
          ))}
        </div>
      )}
      {error && <div className={styles.none}>{t("加载失败：{0}", error)}</div>}
      {events === null ? (
        <div className={styles.none}>
          <Spinner size={14} />
        </div>
      ) : events.length === 0 ? (
        <div className={styles.none}>{t("还没有记录")}</div>
      ) : (
        <div className={styles.rows}>
          {events.map((e) => {
            const k = eventKind(e.kind);
            const details = eventDetails(e);
            return (
              <div key={eventKey(e)} className={styles.row}>
                <span className={styles.when}>{eventTime(e.tsMs)}</span>
                <span className={styles.kind} data-tone={k.tone}>
                  {k.label}
                </span>
                <div className={styles.main}>
                  <span className={e.resource ? styles.what : styles.command}>{eventTitle(e)}</span>
                  {details && <span className={styles.meta}>{details}</span>}
                </div>
              </div>
            );
          })}
        </div>
      )}
      {hasMore && events && events.length > 0 && (
        <button className={styles.more} disabled={loadingMore} onClick={() => void loadMore()}>
          {loadingMore ? <Spinner size={14} /> : t("加载更早的")}
        </button>
      )}
    </section>
  );
}
