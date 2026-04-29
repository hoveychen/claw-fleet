import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useSessionsStore, useUIStore } from "../store";
import type { DecisionHistoryRecord } from "../types";
import { DecisionHistory } from "./DecisionHistory";
import styles from "./LiteDecisionHistory.module.css";

interface Props {
  sessionId: string;
}

// Standalone decision-history view for lite mode. Owns its own fetch and back
// navigation — does NOT route through SessionDetail / useDetailStore, so the
// view is independent of whatever session might also be opened in the drawer.
export function LiteDecisionHistory({ sessionId }: Props) {
  const { t } = useTranslation();
  const setLiteDecisionHistorySessionId = useUIStore(
    (s) => s.setLiteDecisionHistorySessionId,
  );
  const session = useSessionsStore((s) =>
    s.sessions.find((sess) => sess.id === sessionId),
  );

  const [records, setRecords] = useState<DecisionHistoryRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(false);
    invoke<DecisionHistoryRecord[]>("list_session_decisions", {
      sessionId,
      jsonlPath: session?.jsonlPath ?? null,
    })
      .then((r) => {
        if (cancelled) return;
        setRecords(r ?? []);
        setLoading(false);
      })
      .catch(() => {
        if (cancelled) return;
        setRecords([]);
        setError(true);
        setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [sessionId, session?.jsonlPath]);

  const close = () => setLiteDecisionHistorySessionId(null);

  const title =
    session?.aiTitle ?? session?.workspaceName ?? sessionId.slice(0, 8);

  return (
    <div className={styles.root}>
      <div className={styles.header}>
        <button type="button" className={styles.back} onClick={close}>
          <span className={styles.back_chevron}>‹</span>
          <span>{t("lite_decision_history.back", "Back")}</span>
        </button>
        <span className={styles.title}>
          {t("decision_history.title")}
          <span className={styles.subtitle}>· {title}</span>
        </span>
      </div>
      <div className={styles.scroll}>
        {loading ? (
          <div className={styles.status}>
            {t("lite_decision_history.loading", "Loading…")}
          </div>
        ) : error ? (
          <div className={styles.status}>
            {t("lite_decision_history.error", "Failed to load decisions.")}
          </div>
        ) : (
          <DecisionHistory records={records} mode="tab" />
        )}
      </div>
    </div>
  );
}
