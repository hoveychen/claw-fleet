import { useMemo, useState } from "react";
import { ChevronDown, ChevronUp } from "lucide-react";
import { HistoryLayer } from "../useNavStack";
import { DecisionsView, KIND_LABEL } from "./DecisionsView";
import { t } from "../i18n";
import type { FleetTransport } from "../transport";
import type { PendingDecision, SessionInfo } from "../types";
import styles from "./DecisionDrawer.module.css";

interface Props {
  decisions: PendingDecision[];
  client: FleetTransport | null;
  connected: boolean;
  agentOnline: boolean;
  decisionsLoaded: boolean;
  workspaceOf: (sessionId: string) => SessionInfo | undefined;
  onAnswered: (id: string) => void;
  onOpenSession: (sessionId: string) => void;
}

/** Global decision surface that floats above whatever page the boss is on (a
 *  wiki doc, a task list, a session detail) so a card can be answered without
 *  leaving — and once answered, the page underneath is exactly where they left
 *  it. Mounted by App only while cards are pending AND the plain 决策 tab isn't
 *  already showing them, so it never duplicates the tab's own list.
 *
 *  Collapsed it is a compact peek bar that auto-rises above the tab bar when a
 *  card arrives (no screen hijack while reading); tapping expands it into a
 *  bottom sheet whose body reuses the same DecisionsView the 决策 tab renders. */
export function DecisionDrawer(props: Props) {
  const { decisions, workspaceOf, onOpenSession } = props;
  const [expanded, setExpanded] = useState(false);
  const count = decisions.length;

  // Front card = earliest arrived, matching DecisionsView's queue order, so the
  // peek summarises the same card the expanded sheet focuses first.
  const front = useMemo(
    () => [...decisions].sort((a, b) => a.arrivedAt - b.arrivedAt)[0],
    [decisions],
  );
  const frontWs = front
    ? front.request.workspaceName ||
      workspaceOf(front.request.sessionId)?.workspaceName ||
      "Fleet"
    : "";
  const frontKind = front ? t(KIND_LABEL[front.kind] ?? front.kind) : "";

  // Opening a session from a card must not bury the session detail (z-index 30)
  // under the drawer (z-index 45) — collapse to the peek first so the detail
  // shows with just the low-profile bar above it.
  const openSession = (id: string) => {
    setExpanded(false);
    onOpenSession(id);
  };

  if (expanded) {
    return (
      <>
        {/* Back / swipe-back collapses the sheet before it pops any page layer. */}
        <HistoryLayer onBack={() => setExpanded(false)} />
        <div
          className={styles.scrim}
          onClick={() => setExpanded(false)}
          aria-hidden="true"
        />
        <div className={styles.sheet} role="dialog" aria-label={t("决策")}>
          <button
            className={styles.grabberRow}
            onClick={() => setExpanded(false)}
            aria-label={t("收起")}
          >
            <span className={styles.grabber} />
          </button>
          <div className={styles.sheetHead}>
            <span className={styles.sheetTitle}>
              {t("决策")}
              {count > 1 ? ` · ${count}` : ""}
            </span>
            <button
              className={styles.collapseBtn}
              onClick={() => setExpanded(false)}
              aria-label={t("收起")}
            >
              <ChevronDown size={18} />
            </button>
          </div>
          <div className={styles.sheetBody}>
            <DecisionsView {...props} onOpenSession={openSession} />
          </div>
        </div>
      </>
    );
  }

  return (
    <button
      className={styles.peek}
      onClick={() => setExpanded(true)}
      aria-label={t("查看待处理决策")}
    >
      <span className={styles.peekDot} />
      <span className={styles.peekMain}>
        <span className={styles.peekKind}>{frontKind}</span>
        {frontWs && <span className={styles.peekWs}>{frontWs}</span>}
      </span>
      {count > 1 && <span className={styles.peekCount}>{t("共 {0} 张", count)}</span>}
      <ChevronUp size={18} className={styles.peekChevron} />
    </button>
  );
}
