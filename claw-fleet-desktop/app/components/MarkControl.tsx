import { useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import type { SessionInfo, SessionMark } from "../types";
// Reuse the History rail's stylesheet so the row-hover reveal for the "done"
// dot (`.row_wrap:hover .mark_dot[data-mark="done"]`) resolves to the same
// hashed class name whether the selector is written here or there.
import styles from "./HistoryView.module.css";

/** Click cycles through the three buckets: unmarked → pending → done →
 *  unmarked. Unmarked is `null` on the wire (clears the side-channel file). */
function nextMark(cur: SessionMark | null): SessionMark | null {
  if (cur === null) return "pending";
  if (cur === "pending") return "done";
  return null; // done → back to unmarked
}

/**
 * The manual review dot on a History row — a second axis orthogonal to the
 * run-status dot on the left. Red = new / needs review (unmarked), blue = I've
 * seen it but it's not finished (pending), no visible dot = done. Clicking
 * cycles to the next state and persists it via the backend.
 */
export function MarkControl({ session }: { session: SessionInfo }) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState(false);
  // Optimistic override so the dot reacts on click; the scanner re-enriches
  // `userMark` within a few seconds and converges to the same value. `undefined`
  // means "no override — trust the scanned value".
  const [override, setOverride] = useState<SessionMark | null | undefined>(
    undefined,
  );
  const mark: SessionMark | null =
    override !== undefined ? override : (session.userMark ?? null);
  const state = mark ?? "new";

  const onClick = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (busy) return;
    const next = nextMark(mark);
    setBusy(true);
    setOverride(next);
    try {
      await invoke("set_session_mark", {
        sessionId: session.id,
        workspacePath: session.workspacePath,
        mark: next,
      });
    } catch {
      setOverride(mark); // revert the optimistic flip on failure
    } finally {
      setBusy(false);
    }
  };

  const title =
    state === "new"
      ? t("history.mark_new", "未 review — 点击标为「进行中」")
      : state === "pending"
        ? t("history.mark_pending", "进行中 — 点击标为「已完成」")
        : t("history.mark_done", "已完成 — 点击清除标记");

  return (
    <button
      type="button"
      className={styles.mark_dot}
      data-mark={state}
      onClick={onClick}
      title={title}
      aria-label={title}
    />
  );
}
