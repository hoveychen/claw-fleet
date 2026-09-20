import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { AlertTriangle, Radar } from "lucide-react";
import type { SessionInfo } from "../types";
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
 * owns (the detail header's meta row), instead of claiming a line of their own.
 */
export function WatchStatusRow({
  session,
  inline = false,
}: {
  session: SessionInfo;
  inline?: boolean;
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
        // A non-zero streak means the `until` command cannot run at all, so the
        // poll count is not reassuring — it is the measure of how long this
        // session has been waiting for something that will never happen.
        const broken = (w.structuralFailStreak ?? 0) > 0;
        const title = [
          w.note ?? undefined,
          broken
            ? t("card.tip_watch_broken", {
                count: w.structuralFailStreak,
                reason: w.lastStderr || "—",
              })
            : t("card.tip_watch", { elapsed, count: w.pollCount, poll: w.pollSecs }),
        ]
          .filter(Boolean)
          .join(" — ");
        return (
          <span
            key={w.id}
            className={broken ? styles.watch_chip_broken : styles.watch_chip}
            title={title}
          >
            {broken ? <AlertTriangle size={11} /> : <Radar size={11} />}
            {broken
              ? t("card.watch_chip_broken", { count: w.structuralFailStreak })
              : t("card.watch_chip", { elapsed, count: w.pollCount })}
          </span>
        );
      })}
    </>
  );

  return inline ? chips : <div className={styles.watch_row}>{chips}</div>;
}
