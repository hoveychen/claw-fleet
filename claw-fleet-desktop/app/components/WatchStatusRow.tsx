import { useEffect, useState, type SyntheticEvent } from "react";
import { useTranslation } from "react-i18next";
import { AlertTriangle, Hourglass, Radar } from "lucide-react";
import type { SessionInfo } from "../types";
import type { Phase, WatchSummary, WatchTarget } from "../generated/types";
import { useSessionsStore, useUIStore } from "../store";
import { watchProgressView, watchRing } from "../../../shared-ts/watchProgress";
import { WatchRing } from "./WatchRing";
import styles from "./WatchStatusRow.module.css";

/** Human "1h02m" / "3m40s" / "12s" from a millisecond span. Mirrors
 *  SessionCard's formatDuration; kept local so the chip has no cross-import. */
function formatElapsed(ms: number): string {
  if (ms <= 0) return "0s";
  const totalSec = Math.floor(ms / 1000);
  const h = Math.floor(totalSec / 3600);
  const m = Math.floor((totalSec % 3600) / 60);
  const s = totalSec % 60;
  if (h > 0) return `${h}h${m.toString().padStart(2, "0")}m`;
  if (m > 0) return `${m}m${s.toString().padStart(2, "0")}s`;
  return `${s}s`;
}

/**
 * Chip(s) showing this session's active `fleet watch`es — what it is waiting on,
 * how long it has been waiting (elapsed since the watch was registered), and how
 * many times the `--until` condition has been polled. A watch that fires or is
 * stopped deletes its record, so the chip disappears on its own.
 *
 * Callers must guard on `session.watches?.length` being truthy.
 *
 * `inline` drops the row wrapper so the chips can sit in a chip row someone else
 * owns, instead of claiming a line of their own. `stacked` lays them out one per
 * line with the note allowed to wrap, for a narrow column (the session's right
 * rail).
 */
export function WatchStatusRow({
  session,
  inline = false,
  stacked = false,
}: {
  session: SessionInfo;
  inline?: boolean;
  stacked?: boolean;
}) {
  const { t } = useTranslation();
  // Re-tick once a second so elapsed climbs live even between session polls.
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, []);

  const watches = session.watches ?? [];
  if (watches.length === 0) return null;

  const chips = (
    <>
      {watches.map((w) => {
        const elapsed = formatElapsed(now - w.created);
        if (w.targets && w.targets.length > 0) {
          return <TargetWatchChips key={w.id} watch={w} elapsed={elapsed} />;
        }
        // A non-zero streak means the `until` command cannot run at all, so the
        // poll count is not reassuring — it is the measure of how long this
        // session has been waiting for something that will never happen.
        const broken = (w.structuralFailStreak ?? 0) > 0;
        const pv = watchProgressView(w, now);
        const ring = watchRing(pv);
        const title = [
          w.note ?? undefined,
          broken
            ? t("card.tip_watch_broken", {
                count: w.structuralFailStreak,
                reason: w.lastStderr || "—",
              })
            : t("card.tip_watch", { elapsed, count: w.pollCount, poll: w.pollSecs }),
          pv.kind === "reported"
            ? t("card.tip_watch_progress", {
                progress: pv.text,
                since: formatElapsed(now - (w.progressChangedAt ?? w.created)),
              })
            : undefined,
        ]
          .filter(Boolean)
          .join(" — ");
        return (
          <span
            key={w.id}
            className={broken ? styles.watch_chip_broken : styles.watch_chip}
            title={title}
          >
            {broken ? (
              <AlertTriangle size={11} />
            ) : ring ? (
              <WatchRing size={12} fraction={ring.fraction} alarming={ring.alarming} />
            ) : (
              <Radar size={11} />
            )}
            {/* The note is the only answer to "waiting on what?", so it goes on
                the chip itself — a tooltip never shows on touch and is easy to miss. */}
            {w.note && (
              <>
                <span className={styles.watch_note}>{w.note}</span>
                <span className={styles.note_sep} aria-hidden>·</span>
              </>
            )}
            {broken ? (
              t("card.watch_chip_broken", { count: w.structuralFailStreak })
            ) : pv.kind === "reported" ? (
              <>
                <span className={styles.watch_progress}>{pv.text}</span>
                {pv.stalledMs !== null && (
                  <span className={styles.watch_stalled}>
                    {t("card.watch_stalled", { duration: formatElapsed(pv.stalledMs) })}
                  </span>
                )}
                <span aria-hidden>·</span>
                {elapsed}
              </>
            ) : pv.kind === "time" ? (
              <>
                {pv.overdueMs !== null ? (
                  <span className={styles.watch_stalled}>
                    {t("card.watch_overdue", { elapsed, over: formatElapsed(pv.overdueMs) })}
                  </span>
                ) : (
                  t("card.watch_chip_time", { elapsed, expected: formatElapsed(pv.expectedMs) })
                )}
              </>
            ) : (
              t("card.watch_chip", { elapsed, count: w.pollCount })
            )}
          </span>
        );
      })}
    </>
  );

  if (inline) return chips;
  return <div className={stacked ? styles.watch_stack : styles.watch_row}>{chips}</div>;
}

/** Short label for a session id: its live AI title, else a short id. */
function useSessionLabel() {
  const sessions = useSessionsStore((s) => s.sessions);
  return (sid: string) => {
    const s = sessions.find((x) => x.id === sid);
    return s?.aiTitle || sid.replace(/^session-/, "").slice(0, 8);
  };
}

function openSession(e: SyntheticEvent, sid: string) {
  // The chip sits on a clickable card; opening the target must not also
  // select the card underneath.
  e.stopPropagation();
  useUIStore.getState().requestOpenTask(sid);
}

const PHASE_CLASS: Record<Phase, string> = {
  running: styles.dot_running,
  parked: styles.dot_parked,
  blocked: styles.dot_blocked,
  unknown: styles.dot_unknown,
  done: styles.dot_done,
  failed: styles.dot_failed,
};

/**
 * A session watch (`fleet watch --sessions`): one summary chip (note, how many
 * targets are done, all/any, elapsed) followed by one clickable chip per
 * target with a phase dot. A target that handed off is shown — and opened —
 * as the live end of its chain, since that is the session the verdict is about.
 */
function TargetWatchChips({ watch: w, elapsed }: { watch: WatchSummary; elapsed: string }) {
  const { t } = useTranslation();
  const labelOf = useSessionLabel();
  const targets = w.targets ?? [];
  const done = targets.filter((x) => x.phase === "done").length;
  const mode = w.waitMode === "any" ? t("card.watch_mode_any") : t("card.watch_mode_all");
  return (
    <>
      <span className={styles.watch_chip} title={w.note ?? undefined}>
        <Radar size={11} />
        {w.note && (
          <>
            <span className={styles.watch_note}>{w.note}</span>
            <span className={styles.note_sep} aria-hidden>·</span>
          </>
        )}
        {t("card.watch_targets_chip", { done, total: targets.length, mode, elapsed })}
      </span>
      {targets.map((x) => (
        <TargetChip key={x.id} target={x} label={labelOf(x.effectiveId ?? x.id)} />
      ))}
    </>
  );
}

function TargetChip({ target: x, label }: { target: WatchTarget; label: string }) {
  const { t } = useTranslation();
  const sid = x.effectiveId ?? x.id;
  const phase: Phase = x.phase ?? "unknown";
  const title = [
    t(`card.watch_phase_${phase}`),
    x.detail || undefined,
    x.effectiveId ? t("card.watch_target_chain", { from: x.id.slice(0, 8), to: x.effectiveId.slice(0, 8) }) : undefined,
    x.source ?? undefined,
  ]
    .filter(Boolean)
    .join(" — ");
  return (
    <span
      className={styles.target_chip}
      role="button"
      tabIndex={0}
      title={title}
      data-phase={phase}
      onClick={(e) => openSession(e, sid)}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") openSession(e, sid);
      }}
    >
      <span className={`${styles.dot} ${PHASE_CLASS[phase]}`} aria-hidden />
      <span className={styles.target_label}>{label}</span>
    </span>
  );
}

/**
 * The reverse mark on a session other sessions are waiting for: "waited on by
 * N", with each waiter one click away. Callers guard on `session.waitedBy?.length`.
 */
export function WaitedByRow({
  session,
  inline = false,
  stacked = false,
}: {
  session: SessionInfo;
  inline?: boolean;
  stacked?: boolean;
}) {
  const { t } = useTranslation();
  const labelOf = useSessionLabel();
  const edges = session.waitedBy ?? [];
  const waiters = [...new Set(edges.map((e) => e.sessionId))];
  if (waiters.length === 0) return null;
  const tip = edges
    .map((e) => `${labelOf(e.sessionId)}${e.note ? ` — ${e.note}` : ""}`)
    .join("\n");
  const chips = (
    <>
      <span className={styles.waited_chip} title={tip}>
        <Hourglass size={11} />
        {t("card.waited_by_chip", { count: waiters.length })}
      </span>
      {waiters.map((sid) => (
        <span
          key={sid}
          className={styles.target_chip}
          role="button"
          tabIndex={0}
          title={t("card.tip_waited_by", { session: sid })}
          onClick={(e) => openSession(e, sid)}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") openSession(e, sid);
          }}
        >
          <span className={styles.target_label}>{labelOf(sid)}</span>
        </span>
      ))}
    </>
  );
  if (inline) return chips;
  return <div className={stacked ? styles.watch_stack : styles.watch_row}>{chips}</div>;
}
