import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { LogicalSize } from "@tauri-apps/api/dpi";
import { useEffect, useMemo, useRef } from "react";
import { useTranslation } from "react-i18next";
import { useSessionsStore } from "../store";
import type { SessionInfo } from "../types";
import { StatusBadge, AgentSourceIcon, SubagentTypeIcon, formatModel } from "./SessionCard";
import styles from "./TrayPanel.module.css";

// ── Helpers ─────────────────────────────────────────────────────────────────

function isActive(s: SessionInfo): boolean {
  return ["thinking", "executing", "streaming", "processing", "waitingInput", "active", "delegating"].includes(s.status);
}

function timeAgo(ms: number, t: (k: string, opts?: Record<string, unknown>) => string): string {
  const diff = Date.now() - ms;
  if (diff < 60_000) return t("just_now");
  if (diff < 3_600_000) return t("m_ago", { n: Math.floor(diff / 60_000) });
  if (diff < 86_400_000) return t("h_ago", { n: Math.floor(diff / 3_600_000) });
  return t("d_ago", { n: Math.floor(diff / 86_400_000) });
}

// ── Context bar ─────────────────────────────────────────────────────────────

function ContextBar({ percent }: { percent: number | null }) {
  if (percent == null) return null;
  const pct = Math.round(percent * 100);
  const cls = pct >= 90 ? styles.critical : pct >= 70 ? styles.high : "";
  return (
    <div className={styles.context_bar_wrap}>
      <div className={styles.context_bar_track}>
        <div className={`${styles.context_bar_fill} ${cls}`} style={{ width: `${Math.min(pct, 100)}%` }} />
      </div>
      <span className={styles.context_bar_label}>{pct}%</span>
    </div>
  );
}

// ── Session item ────────────────────────────────────────────────────────────

function SessionItem({ session, isSub }: { session: SessionInfo; isSub: boolean }) {
  const { t } = useTranslation();

  const handleClick = () => {
    invoke("show_main_window").catch(() => {});
    emit("open-session", session.jsonlPath).catch(() => {});
    invoke("toggle_tray_panel", { visible: false }).catch(() => {});
  };

  const displayTitle = session.aiTitle ?? session.agentDescription ?? null;

  return (
    <div
      className={`${styles.session_item} ${isSub ? styles.is_subagent : ""}`}
      onClick={handleClick}
    >
      <div className={styles.session_row}>
        {isSub && (
          <span className={styles.session_subagent_prefix}>
            <SubagentTypeIcon type={session.agentType} />
          </span>
        )}
        <span className={styles.session_name} title={session.workspacePath}>
          {session.workspaceName}
        </span>
        <span className={styles.session_badge}>
          <StatusBadge status={session.status} />
        </span>
      </div>
      {displayTitle && (
        <div className={styles.session_ai_title} title={displayTitle}>
          {displayTitle}
        </div>
      )}
      <ContextBar percent={session.contextPercent} />
      <div className={styles.session_meta}>
        <AgentSourceIcon source={session.agentSource} />
        {session.model && (
          <span className={styles.session_model}>{formatModel(session.model)}</span>
        )}
        {session.tokenSpeed >= 0.5 && (
          <span className={styles.session_speed}>{session.tokenSpeed.toFixed(1)} tok/s</span>
        )}
        <span className={styles.session_time}>{timeAgo(session.lastActivityMs, t)}</span>
      </div>
    </div>
  );
}

// ── Usage bar row ───────────────────────────────────────────────────────────

export interface UsageBarData {
  label: string;
  utilization: number;
}

export interface UsageSummary {
  source: string;
  bars: UsageBarData[];
}

function UsageRow({ summary }: { summary: UsageSummary }) {
  const sourceLabel: Record<string, string> = {
    claude: "Claude",
    cursor: "Cursor",
    codex: "Codex",
    openclaw: "OpenClaw",
  };

  const maxBar = summary.bars.reduce((a, b) => (b.utilization > a.utilization ? b : a), summary.bars[0]);
  if (!maxBar) return null;
  const pct = Math.round(maxBar.utilization * 100);
  const cls = pct >= 90 ? styles.usage_crit : pct >= 70 ? styles.usage_warn : "";

  return (
    <div className={styles.usage_row}>
      <span className={styles.usage_source}>
        <AgentSourceIcon source={summary.source === "claude" ? "claude-code" : summary.source} />
        {sourceLabel[summary.source] ?? summary.source}
      </span>
      <div className={styles.usage_bar_track}>
        <div className={`${styles.usage_bar_fill} ${cls}`} style={{ width: `${Math.min(pct, 100)}%` }} />
      </div>
      <span className={styles.usage_pct}>{pct}%</span>
    </div>
  );
}

// ── Group sessions: main → subagents ────────────────────────────────────────

interface SessionGroup {
  main: SessionInfo;
  subs: SessionInfo[];
}

function useGroupedSessions(): SessionGroup[] {
  const sessions = useSessionsStore((s) => s.sessions);
  return useMemo(() => {
    const active = sessions.filter(isActive);
    const mains = active.filter((s) => !s.isSubagent);
    const subsByParent = new Map<string, SessionInfo[]>();
    for (const s of active) {
      if (s.isSubagent && s.parentSessionId) {
        const list = subsByParent.get(s.parentSessionId) ?? [];
        list.push(s);
        subsByParent.set(s.parentSessionId, list);
      }
    }
    // Sort mains by most recent activity
    mains.sort((a, b) => b.lastActivityMs - a.lastActivityMs);
    return mains.map((m) => ({
      main: m,
      subs: (subsByParent.get(m.id) ?? []).sort((a, b) => b.lastActivityMs - a.lastActivityMs),
    }));
  }, [sessions]);
}

// ── Main TrayPanel ──────────────────────────────────────────────────────────

interface TrayPanelProps {
  usageSummaries: UsageSummary[];
}

export function TrayPanel({ usageSummaries }: TrayPanelProps) {
  const { t } = useTranslation();
  const groups = useGroupedSessions();
  const panelRef = useRef<HTMLDivElement>(null);

  const totalActive = groups.reduce((n, g) => n + 1 + g.subs.length, 0);
  const headerText = totalActive > 0
    ? t("tray.active_agents", { n: totalActive })
    : t("tray.no_active_agents");

  // Dynamically resize the Tauri window to match content height.
  useEffect(() => {
    const el = panelRef.current;
    if (!el) return;
    const ro = new ResizeObserver((entries) => {
      const height = entries[0]?.contentRect.height;
      if (height && height > 0) {
        const win = getCurrentWebviewWindow();
        win.setSize(new LogicalSize(360, Math.min(Math.ceil(height) + 2, 520))).catch(() => {});
      }
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const handleOpenDashboard = () => {
    invoke("show_main_window").catch(() => {});
    invoke("toggle_tray_panel", { visible: false }).catch(() => {});
  };

  const handleQuit = () => {
    invoke("quit_app").catch(() => {});
  };

  return (
    <div className={styles.panel} ref={panelRef}>
      {/* Header */}
      <div className={styles.header}>
        <span className={styles.title}>{headerText}</span>
        <div className={styles.header_actions}>
          <button className={styles.header_btn} onClick={handleOpenDashboard} title={t("tray.show")}>
            Dashboard
          </button>
        </div>
      </div>

      {/* Session list — grouped by main agent */}
      {groups.length > 0 && (
        <div className={styles.session_list}>
          {groups.map((g) => (
            <div key={g.main.id} className={styles.session_group}>
              <SessionItem session={g.main} isSub={false} />
              {g.subs.map((sub) => (
                <SessionItem key={sub.id} session={sub} isSub={true} />
              ))}
            </div>
          ))}
        </div>
      )}

      {/* Usage — always shown */}
      {usageSummaries.length > 0 && (
        <div className={styles.usage_section}>
          <div className={styles.usage_header}>{t("tray.usage")}</div>
          {usageSummaries.map((s) => (
            <UsageRow key={s.source} summary={s} />
          ))}
        </div>
      )}

      {/* Footer */}
      <div className={styles.footer}>
        <button className={styles.footer_btn} onClick={handleOpenDashboard}>
          {t("tray.show")}
        </button>
        <button className={`${styles.footer_btn} ${styles.footer_btn_quit}`} onClick={handleQuit}>
          {t("tray.quit")}
        </button>
      </div>
    </div>
  );
}
