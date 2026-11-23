// Status card under a `fleet__job run` step in session details: the resources
// the job asked for and where it is now — its place in the queue, holding them,
// or finished with an exit code — with how long it has waited / run. Tapping it
// opens the resources page. Mirrors the desktop's `JobStatusPanel`; the state
// logic both share lives in `shared-ts/jobCard.ts`.
//
// The relay strips the result body, so the job id arrives as the result's
// `_job` digest (`job_run_summary` in mobile_relay.rs), and the call's
// `command` / `need` / `note` survive the input whitelist for this card.

import { createContext, useContext, useEffect, useMemo, useState } from "react";

import { t } from "../i18n";
import { waitLines } from "../resources";
import type { FleetTransport } from "../transport";
import type { ProcRecord, ResourceSnapshot } from "../generated/types";
import {
  createSnapPoller,
  fmtJobDuration,
  jobLive,
  jobLiveSettled,
  jobNeedsRecord,
  type JobLive,
  type JobRunCall,
} from "../../../shared-ts/jobCard";
import styles from "./JobCard.module.css";

const SNAP_POLL_MS = 4000;
const REC_POLL_MS = 6000;

/** Opens the resources page. Provided by App around session details; null
 *  where there is no page to open (the card then is not tappable). */
export const OpenResourcesContext = createContext<(() => void) | null>(null);

/** One queue poll per host connection, shared by every job card on screen —
 *  each card polling on its own would multiply relay round trips. */
const pollers = new WeakMap<FleetTransport, ReturnType<typeof createSnapPoller<ResourceSnapshot>>>();
function pollerFor(client: FleetTransport) {
  let p = pollers.get(client);
  if (!p) {
    p = createSnapPoller(() => client.request<ResourceSnapshot>("resource_list"), SNAP_POLL_MS);
    pollers.set(client, p);
  }
  return p;
}

function useJobLive(client: FleetTransport | null, id: string | null) {
  const [snap, setSnap] = useState<ResourceSnapshot | null>(null);
  const [rec, setRec] = useState<ProcRecord | null>(null);
  const [recMissing, setRecMissing] = useState(false);
  const [now, setNow] = useState(() => Date.now());

  const live = useMemo<JobLive>(
    () => (id ? jobLive(id, snap, rec, recMissing, now) : { kind: "gone" }),
    [id, snap, rec, recMissing, now],
  );
  const settled = jobLiveSettled(live);

  useEffect(() => {
    if (!client || !id || settled) return;
    return pollerFor(client).subscribe((s) => {
      setSnap(s);
      setNow(Date.now());
    });
  }, [client, id, settled]);

  const needRec = !!client && !!id && !settled && jobNeedsRecord(id, snap);
  useEffect(() => {
    if (!needRec || !client || !id) return;
    let alive = true;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const load = async () => {
      try {
        const r = await client.request<ProcRecord>("proc_get", { id });
        if (!alive) return;
        setRec(r);
        setNow(Date.now());
        if (r.status !== "exited") timer = setTimeout(load, REC_POLL_MS);
      } catch {
        // No record (cleared, or a host without `proc_get`): show "ended".
        if (alive) setRecMissing(true);
      }
    };
    void load();
    return () => {
      alive = false;
      if (timer) clearTimeout(timer);
    };
  }, [needRec, client, id]);

  useEffect(() => {
    if (settled || live.kind === "loading") return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [settled, live.kind]);

  return { live, snap };
}

function StateLine({ live, snap, id }: { live: JobLive; snap: ResourceSnapshot | null; id: string }) {
  const d = fmtJobDuration;
  switch (live.kind) {
    case "loading":
      return <span className={styles.dim}>{t("读取状态…")}</span>;
    case "queued": {
      const q = snap?.queue.find((e) => e.request.id === id);
      return (
        <>
          <span className={`${styles.badge} ${styles.queued}`}>
            {live.position != null ? t("排队第 {0} 位 / 共 {1}", live.position, live.queueLen ?? 0) : t("排队中")}
          </span>
          <span className={styles.clock}>{t("已等 {0}", d(live.waitedMs))}</span>
          {q && snap && waitLines(q, snap).map((l) => <span key={l} className={styles.wait}>{l}</span>)}
        </>
      );
    }
    case "holding":
      return (
        <>
          <span className={`${styles.badge} ${styles.holding}`}>{t("已持有 · 运行中")}</span>
          <span className={styles.clock}>{t("已跑 {0}", d(live.ranMs))}</span>
        </>
      );
    case "running":
      return (
        <>
          <span className={`${styles.badge} ${styles.holding}`}>{t("运行中")}</span>
          <span className={styles.clock}>{t("已跑 {0}", d(live.ranMs))}</span>
        </>
      );
    case "exited": {
      const label =
        live.end === "timeout"
          ? t("排队超时 · 未运行")
          : live.end === "cancelled"
            ? t("已取消 · 未运行")
            : live.end === "rejected"
              ? t("申请被拒 · 未运行")
              : live.exitCode == null
                ? t("已结束")
                : t("已结束 · 退出码 {0}", live.exitCode);
      return (
        <>
          <span className={`${styles.badge} ${live.exitCode === 0 ? styles.ok : styles.fail}`}>{label}</span>
          {live.ranMs != null && <span className={styles.clock}>{t("跑了 {0}", d(live.ranMs))}</span>}
        </>
      );
    }
    case "gone":
      return <span className={`${styles.badge} ${styles.done}`}>{t("已结束")}</span>;
  }
}

export function JobCard({
  call,
  jobId,
  client,
}: {
  call: JobRunCall;
  /** Null while the call is still in flight (the id is in its result). */
  jobId: string | null;
  client: FleetTransport | null;
}) {
  const openResources = useContext(OpenResourcesContext);
  const { live, snap } = useJobLive(client, jobId);
  const hasNeeds = call.needs.length > 0;
  const tappable = hasNeeds && !!openResources;

  const body = (
    <>
      {hasNeeds && (
        <span className={styles.needs}>
          {call.needs.map((n) => (
            <span key={n} className={styles.need}>{n}</span>
          ))}
        </span>
      )}
      <span className={styles.state}>
        {jobId ? <StateLine live={live} snap={snap} id={jobId} /> : <span className={styles.dim}>{t("提交中…")}</span>}
      </span>
    </>
  );
  const state = jobId ? live.kind : "pending";
  if (!tappable) {
    return (
      <div className={styles.card} data-job-state={state}>
        {body}
      </div>
    );
  }
  return (
    <button
      type="button"
      className={`${styles.card} ${styles.tappable}`}
      data-job-state={state}
      onClick={(e) => {
        e.stopPropagation();
        openResources!();
      }}
    >
      {body}
    </button>
  );
}
