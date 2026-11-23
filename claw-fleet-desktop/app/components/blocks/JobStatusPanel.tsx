import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { invoke } from "@tauri-apps/api/core";
import type { ProcRecord, ResourceSnapshot } from "../../generated/types";
import { useUIStore } from "../../store";
import {
  createSnapPoller,
  fmtJobDuration,
  jobLive,
  jobLiveSettled,
  jobNeedsRecord,
  type JobLive,
  type JobRunCall,
  type JobSnapWait,
} from "../../../../shared-ts/jobCard";
import styles from "./JobStatusPanel.module.css";

const SNAP_POLL_MS = 3000;
const REC_POLL_MS = 5000;

/** One queue poll for every job card in the window (see `createSnapPoller`). */
const snapPoller = createSnapPoller(() => invoke<ResourceSnapshot>("resource_snapshot"), SNAP_POLL_MS);

/**
 * Live state of job `id`: the resource queue first, the job's own record for
 * whatever the queue does not place. Stops polling once the job has exited or
 * is gone.
 */
function useJobLive(id: string | null): { live: JobLive; snap: ResourceSnapshot | null } {
  const [snap, setSnap] = useState<ResourceSnapshot | null>(null);
  const [rec, setRec] = useState<ProcRecord | null>(null);
  const [recMissing, setRecMissing] = useState(false);
  const [now, setNow] = useState(() => Date.now());

  const live = useMemo(
    () => (id ? jobLive(id, snap, rec, recMissing, now) : ({ kind: "gone" } as JobLive)),
    [id, snap, rec, recMissing, now],
  );
  const settled = jobLiveSettled(live);

  // Queue snapshot, shared across cards.
  useEffect(() => {
    if (!id || settled) return;
    return snapPoller.subscribe((s) => {
      setSnap(s);
      setNow(Date.now());
    });
  }, [id, settled]);

  // The record, only while the queue does not place the job.
  const needRec = !!id && !settled && jobNeedsRecord(id, snap);
  useEffect(() => {
    if (!needRec || !id) return;
    let alive = true;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const load = async () => {
      try {
        const r = await invoke<ProcRecord>("get_workspace_proc", { id });
        if (!alive) return;
        setRec(r);
        setNow(Date.now());
        if (r.status !== "exited") timer = setTimeout(load, REC_POLL_MS);
      } catch {
        if (alive) setRecMissing(true);
      }
    };
    void load();
    return () => {
      alive = false;
      if (timer) clearTimeout(timer);
    };
  }, [needRec, id]);

  // Tick the clocks between polls so "waited 41s" keeps moving.
  useEffect(() => {
    if (settled || live.kind === "loading") return;
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [settled, live.kind]);

  return { live, snap };
}

function shortCmd(cmd: string, max = 28): string {
  const first = cmd.trim().split("\n")[0] ?? "";
  return first.length > max ? `${first.slice(0, max - 1)}…` : first;
}

function waitText(w: JobSnapWait, snap: ResourceSnapshot | null, t: TFunction): string {
  if (w.reason === "busy") {
    const who = w.by
      .map((id) => shortCmd(snap?.holders.find((h) => h.id === id)?.command ?? id))
      .join("、");
    return t("resources.wait_busy", "等 {{res}}：被 {{who}} 占着", { res: w.resource, who: who || "—" });
  }
  const ahead = w.by
    .map((id) => {
      const pos = snap?.queue.find((q) => q.request.id === id)?.position;
      return pos ? `#${pos}` : id;
    })
    .join("、");
  return t("resources.wait_reserved", "等 {{res}}：空着，但留给了排在前面的 {{who}}", { res: w.resource, who: ahead });
}

function StateLine({ live, snap, t }: { live: JobLive; snap: ResourceSnapshot | null; t: TFunction }) {
  const d = fmtJobDuration;
  switch (live.kind) {
    case "loading":
      return <span className={styles.dim}>{t("fleet.job_card.loading", "读取状态…")}</span>;
    case "queued":
      return (
        <>
          <span className={`${styles.badge} ${styles.badge_queued}`}>
            {live.position != null
              ? t("fleet.job_card.queued_pos", "排队第 {{pos}} 位 / 共 {{len}}", { pos: live.position, len: live.queueLen })
              : t("fleet.job_card.queued", "排队中")}
          </span>
          <span className={styles.clock}>{t("fleet.job_card.waited", "已等 {{d}}", { d: d(live.waitedMs) })}</span>
          {live.waitOn.map((w) => (
            <span key={w.resource} className={styles.wait}>{waitText(w, snap, t)}</span>
          ))}
        </>
      );
    case "holding":
      return (
        <>
          <span className={`${styles.badge} ${styles.badge_holding}`}>{t("fleet.job_card.holding", "已持有 · 运行中")}</span>
          <span className={styles.clock}>{t("fleet.job_card.ran", "已跑 {{d}}", { d: d(live.ranMs) })}</span>
          {live.queuedMs >= 1000 && (
            <span className={styles.dim}>{t("fleet.job_card.queued_first", "先排队 {{d}}", { d: d(live.queuedMs) })}</span>
          )}
        </>
      );
    case "running":
      return (
        <>
          <span className={`${styles.badge} ${styles.badge_holding}`}>{t("fleet.job_card.running", "运行中")}</span>
          <span className={styles.clock}>{t("fleet.job_card.ran", "已跑 {{d}}", { d: d(live.ranMs) })}</span>
        </>
      );
    case "exited": {
      const ok = live.exitCode === 0;
      const label = live.end
        ? t(`fleet.job_card.end_${live.end}`, live.end)
        : live.exitCode == null
          ? t("fleet.job_card.exited_unknown", "已结束")
          : t("fleet.job_card.exited", "已结束 · 退出码 {{code}}", { code: live.exitCode });
      return (
        <>
          <span className={`${styles.badge} ${ok ? styles.badge_ok : styles.badge_fail}`}>{label}</span>
          {live.ranMs != null && (
            <span className={styles.clock}>{t("fleet.job_card.took", "跑了 {{d}}", { d: d(live.ranMs) })}</span>
          )}
          {live.queuedMs != null && live.queuedMs >= 1000 && (
            <span className={styles.dim}>{t("fleet.job_card.queued_for", "排队 {{d}}", { d: d(live.queuedMs) })}</span>
          )}
        </>
      );
    }
    case "gone":
      return <span className={`${styles.badge} ${styles.badge_done}`}>{t("fleet.job_card.gone", "已结束")}</span>;
  }
}

/**
 * Always-visible status strip under a `fleet__job run` card: the resources the
 * job asked for and where it is now — its place in the queue, holding them, or
 * finished with an exit code — plus how long it has waited / run. Clicking it
 * opens the resources page, where the whole queue is.
 *
 * `jobId` is null until the call's result arrives (the id is only in the
 * result text); the strip then shows just the requested resources.
 */
export function JobStatusPanel({ call, jobId }: { call: JobRunCall; jobId: string | null }) {
  const { t } = useTranslation();
  const setViewMode = useUIStore((s) => s.setViewMode);
  const { live, snap } = useJobLive(jobId);
  const hasNeeds = call.needs.length > 0;

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
        {jobId ? (
          <StateLine live={live} snap={snap} t={t} />
        ) : (
          <span className={styles.dim}>{t("fleet.job_card.starting", "提交中…")}</span>
        )}
      </span>
      {jobId && <span className={styles.id}>{jobId}</span>}
    </>
  );
  const state = jobId ? live.kind : "pending";
  // Only a job that queued has anything on the resources page.
  if (!hasNeeds) {
    return (
      <div className={styles.root} data-job-state={state}>
        {body}
      </div>
    );
  }
  return (
    <button
      type="button"
      className={`${styles.root} ${styles.clickable}`}
      data-job-state={state}
      onClick={(e) => {
        e.stopPropagation();
        setViewMode("resources");
      }}
      title={t("fleet.job_card.open_resources", "在资源页查看")}
    >
      {body}
    </button>
  );
}
