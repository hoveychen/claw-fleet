import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useDetailStore, useSessionsStore, useUIStore } from "../store";
import type { AuditEvent, AuditRiskLevel, AuditSummary } from "../types";
import styles from "./AuditView.module.css";

// ── Risk level helpers ──────────────────────────────────────────────────────

const RISK_COLORS: Record<AuditRiskLevel, string> = {
  critical: "#ef4444",
  high: "#f97316",
  medium: "#eab308",
};

const RISK_LABELS: Record<AuditRiskLevel, string> = {
  critical: "CRIT",
  high: "HIGH",
  medium: "MED",
};

// ── Component ───────────────────────────────────────────────────────────────

export function AuditView() {
  const { t } = useTranslation();
  const [summary, setSummary] = useState<AuditSummary | null>(null);
  const [loading, setLoading] = useState(true);
  const [filter, setFilter] = useState<AuditRiskLevel | "all">("all");
  const [selectedEvent, setSelectedEvent] = useState<AuditEvent | null>(null);
  const { sessions } = useSessionsStore();
  const { open } = useDetailStore();
  const { setViewMode } = useUIStore();

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const data = await invoke<AuditSummary>("get_audit_events");
      setSummary(data);
    } catch {
      setSummary({ events: [], totalSessionsScanned: 0 });
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => { load(); }, [load]);

  const filtered = summary?.events.filter(
    (e) => filter === "all" || e.riskLevel === filter
  ) ?? [];

  // Group by session
  const grouped = new Map<string, AuditEvent[]>();
  for (const e of filtered) {
    const key = e.sessionId;
    if (!grouped.has(key)) grouped.set(key, []);
    grouped.get(key)!.push(e);
  }

  // Count by risk level
  const counts: Record<AuditRiskLevel, number> = { critical: 0, high: 0, medium: 0 };
  for (const e of summary?.events ?? []) {
    counts[e.riskLevel]++;
  }

  const navigateToSession = (jsonlPath: string) => {
    const session = sessions.find((s) => s.jsonlPath === jsonlPath);
    if (session) {
      setViewMode("list");
      open(session);
    }
  };

  return (
    <div className={styles.container}>
      {/* Header bar */}
      <div className={styles.header} data-tauri-drag-region>
        <h2 className={styles.title}>{t("audit.panel_title")}</h2>
        <div className={styles.filter_bar}>
          {(["all", "critical", "high", "medium"] as const).map((level) => {
            const count = level === "all"
              ? (summary?.events.length ?? 0)
              : counts[level];
            return (
              <button
                key={level}
                className={`${styles.filter_btn} ${filter === level ? styles.filter_active : ""}`}
                onClick={() => setFilter(level)}
                style={level !== "all" ? { color: RISK_COLORS[level] } : undefined}
              >
                {level === "all" ? t("audit.all") : RISK_LABELS[level]}
                {count > 0 && ` (${count})`}
              </button>
            );
          })}
        </div>
        <button className={styles.refresh_btn} onClick={load} title={t("audit.refresh")}>
          ↻
        </button>
        {summary && (
          <span className={styles.scan_info}>
            {t("audit.scanned", { count: summary.totalSessionsScanned })}
          </span>
        )}
      </div>

      {/* Body: event list + detail panel */}
      <div className={styles.body}>
        {/* Left: event list */}
        <div className={styles.event_list}>
          {loading && (
            <p className={styles.empty}>{t("audit.scanning")}</p>
          )}

          {!loading && filtered.length === 0 && (
            <p className={styles.empty}>{t("audit.no_events")}</p>
          )}

          {!loading && Array.from(grouped.entries()).map(([sessionId, events]) => (
            <div key={sessionId} className={styles.session_group}>
              <div
                className={styles.session_header}
                onClick={() => navigateToSession(events[0].jsonlPath)}
              >
                <span className={styles.session_name}>
                  {events[0].workspaceName}
                </span>
                <span className={styles.session_source}>
                  {events[0].agentSource}
                </span>
                <span className={styles.session_count}>
                  {events.length}
                </span>
              </div>
              {events.map((event, i) => (
                <div
                  key={`${sessionId}-${i}`}
                  className={`${styles.event_row} ${selectedEvent === event ? styles.event_row_selected : ""}`}
                  onClick={() => setSelectedEvent(event)}
                >
                  <span
                    className={styles.risk_badge}
                    style={{ background: `${RISK_COLORS[event.riskLevel]}20`, color: RISK_COLORS[event.riskLevel] }}
                  >
                    {RISK_LABELS[event.riskLevel]}
                  </span>
                  <span className={styles.event_command}>
                    {event.commandSummary}
                  </span>
                  {event.timestamp && (
                    <span className={styles.event_time}>
                      {formatTime(event.timestamp)}
                    </span>
                  )}
                </div>
              ))}
            </div>
          ))}
        </div>

        {/* Right: detail panel */}
        {selectedEvent && (
          <div className={styles.detail_panel}>
            <div className={styles.detail_header}>
              <span
                className={styles.risk_badge}
                style={{ background: `${RISK_COLORS[selectedEvent.riskLevel]}20`, color: RISK_COLORS[selectedEvent.riskLevel] }}
              >
                {RISK_LABELS[selectedEvent.riskLevel]}
              </span>
              <span className={styles.detail_title}>{selectedEvent.toolName}</span>
              <button className={styles.detail_close} onClick={() => setSelectedEvent(null)}>
                ✕
              </button>
            </div>

            <div className={styles.detail_body}>
              <DetailRow label={t("audit.workspace")} value={selectedEvent.workspaceName} />
              <DetailRow label={t("audit.source")} value={selectedEvent.agentSource} />
              {selectedEvent.timestamp && (
                <DetailRow
                  label={t("audit.time")}
                  value={new Date(selectedEvent.timestamp).toLocaleString()}
                />
              )}
              {selectedEvent.riskTags.length > 0 && (
                <div className={styles.detail_row}>
                  <span className={styles.detail_label}>{t("audit.tags")}</span>
                  <span className={styles.detail_value}>
                    {selectedEvent.riskTags.map((tag) => (
                      <span key={tag} className={styles.tag}>{tag}</span>
                    ))}
                  </span>
                </div>
              )}

              <div className={styles.command_section}>
                <div className={styles.command_label}>{t("audit.full_command")}</div>
                <pre className={styles.command_pre}>{selectedEvent.fullCommand}</pre>
              </div>

              <button
                className={styles.goto_btn}
                onClick={() => navigateToSession(selectedEvent.jsonlPath)}
              >
                {t("audit.go_to_session")}
              </button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

// ── Helpers ──────────────────────────────────────────────────────────────────

function DetailRow({ label, value }: { label: string; value: string }) {
  return (
    <div className={styles.detail_row}>
      <span className={styles.detail_label}>{label}</span>
      <span className={styles.detail_value}>{value}</span>
    </div>
  );
}

function formatTime(ts: string): string {
  try {
    const d = new Date(ts);
    return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  } catch {
    return "";
  }
}
