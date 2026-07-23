import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { CheckCircle2, Circle } from "lucide-react";
import type { SessionInfo } from "../types";
import styles from "./SessionRow.module.css";

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
  // Optimistic override so the icon reacts on the click rather than on the
  // round-trip; `set_session_mark` re-stamps `userMark` and emits
  // `sessions-updated`, so the store converges right behind it. `undefined` =
  // no override.
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

/**
 * The mark-all toggle on a relay-group header. Same pending/done axis as
 * MarkControl, but it fans the `set_session_mark` call out across every member
 * of the chain (`members` = the full membership, not just the visible hops).
 * Reads as done only when *all* members are done; clicking marks the whole
 * chain, clicking again clears it. Optimistic like MarkControl; the override is
 * dropped once the scan converges on the same aggregate state.
 */
export function GroupMarkControl({ members }: { members: SessionInfo[] }) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState(false);
  const [override, setOverride] = useState<boolean | undefined>(undefined);
  const allDone = members.length > 0 && members.every((m) => m.userMark === "done");
  const isDone = override !== undefined ? override : allDone;

  useEffect(() => {
    if (override !== undefined && override === allDone) setOverride(undefined);
  }, [allDone, override]);

  const onClick = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (busy) return;
    const next = !isDone;
    setBusy(true);
    setOverride(next);
    try {
      await Promise.all(
        members.map((m) =>
          invoke("set_session_mark", {
            sessionId: m.id,
            workspacePath: m.workspacePath,
            mark: next ? "done" : null,
          }),
        ),
      );
    } catch {
      setOverride(!next); // revert the optimistic flip on failure
    } finally {
      setBusy(false);
    }
  };

  const title = isDone
    ? t("history.group_toggle_done", "整条接力链已完成 — 点击全部改回进行中")
    : t("history.group_toggle_pending", "点击把整条接力链标为已完成");

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
      {/* Same hollow-circle / filled-check glyph as a single row's MarkControl —
          the whole-chain semantics are already carried by the chevron, the 接力
          badge and this button's tooltip, so the mark itself stays visually
          identical to every other row's rather than a distinct double-check. */}
      {isDone ? (
        <CheckCircle2 size={15} strokeWidth={1.8} />
      ) : (
        <Circle size={14} strokeWidth={1.8} />
      )}
    </button>
  );
}
