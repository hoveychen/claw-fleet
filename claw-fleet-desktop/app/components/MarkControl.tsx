import { useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { CheckCircle2, Circle } from "lucide-react";
import type { SessionInfo } from "../types";
import styles from "./HistoryView.module.css";

/**
 * The pending/done toggle on a launchpad row — the manual "have I finished with
 * this?" axis, orthogonal to the read/unread dot (top-left) and the run status
 * (left accent bar). Two states only: unmarked reads as *pending* (hollow
 * circle), `done` is explicit (filled check). Clicking flips between them, in
 * either direction — done can go back to pending. Persisted via `set_session_mark`
 * (`null` clears back to the pending default).
 */
export function MarkControl({ session }: { session: SessionInfo }) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState(false);
  // Optimistic override so the icon reacts on click; the scanner re-enriches
  // `userMark` within a few seconds and converges. `undefined` = no override.
  const [override, setOverride] = useState<boolean | undefined>(undefined);
  const isDone = override !== undefined ? override : session.userMark === "done";

  const onClick = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (busy) return;
    const next = !isDone;
    setBusy(true);
    setOverride(next);
    try {
      await invoke("set_session_mark", {
        sessionId: session.id,
        workspacePath: session.workspacePath,
        // `done` is explicit; clearing to null is the pending default.
        mark: next ? "done" : null,
      });
    } catch {
      setOverride(!next); // revert the optimistic flip on failure
    } finally {
      setBusy(false);
    }
  };

  const title = isDone
    ? t("history.toggle_done", "已完成 — 点击改回进行中")
    : t("history.toggle_pending", "进行中 — 点击标为已完成");

  return (
    <button
      type="button"
      className={styles.mark_toggle}
      data-done={isDone ? "true" : "false"}
      onClick={onClick}
      title={title}
      aria-label={title}
      aria-pressed={isDone}
    >
      {isDone ? (
        <CheckCircle2 size={15} strokeWidth={1.8} />
      ) : (
        <Circle size={14} strokeWidth={1.8} />
      )}
    </button>
  );
}
