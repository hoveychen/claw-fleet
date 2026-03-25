import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import styles from "./UsagePanel.module.css";

const AUTO_REFRESH_INTERVAL_MS = 5 * 60 * 1000;

interface UsageStats {
  utilization: number;
  resets_at: string;
  prev_utilization: number | null;
}

interface AccountInfoData {
  email: string;
  full_name: string;
  organization_name: string;
  plan: string;
  auth_method: string;
  five_hour: UsageStats | null;
  seven_day: UsageStats | null;
  seven_day_sonnet: UsageStats | null;
}

interface CursorUsageItem {
  name: string;
  used: number;
  limit: number | null;
  utilization: number | null;
  resetsAt: string | null;
}

interface CursorAccountInfoData {
  email: string;
  signUpType: string;
  membershipType: string;
  subscriptionStatus: string;
  totalPrompts: number;
  dailyStats: unknown[];
  usage: CursorUsageItem[];
}

interface CodexRateLimitWindow {
  usedPercent: number;
  windowDurationMins?: number | null;
  resetsAt?: number | null;
}

interface CodexUsageItem {
  limitId?: string | null;
  limitName?: string | null;
  planType?: string | null;
  primary?: CodexRateLimitWindow | null;
  secondary?: CodexRateLimitWindow | null;
  credits?: { hasCredits: boolean; unlimited: boolean; balance?: string | null } | null;
}

type TFunc = (key: string, opts?: Record<string, unknown>) => string;

function formatResetIn(resets_at: string, t: TFunc): string {
  const diff = new Date(resets_at).getTime() - Date.now();
  if (diff <= 0) return t("account.resets_soon");
  const h = Math.floor(diff / 3600000);
  const d = Math.floor(h / 24);
  if (d >= 1) return t("account.resets_days", { n: d });
  if (h >= 1) return t("account.resets_hours", { n: h });
  const m = Math.floor(diff / 60000);
  return t("account.resets_mins", { n: m });
}

function formatLastUpdated(ts: number | null, t: TFunc): string {
  if (!ts) return "";
  const diff = Date.now() - ts;
  if (diff < 5000) return t("account.updated_just_now");
  const m = Math.floor(diff / 60000);
  if (m < 1) return t("account.updated_s_ago", { n: Math.floor(diff / 1000) });
  return t("account.updated", { n: m });
}

// ── Generic usage bar (Claude-style: utilization 0–1) ────────────────────────

function UsageBar({ label, stats }: { label: string; stats: UsageStats | null }) {
  const { t } = useTranslation();
  if (!stats) return null;
  const pct = Math.round(stats.utilization * 100);
  const prev =
    stats.prev_utilization !== null && stats.prev_utilization !== undefined
      ? Math.round(stats.prev_utilization * 100)
      : null;

  let trend: "faster" | "slower" | "similar" | null = null;
  if (prev !== null) {
    const diff = pct - prev;
    if (diff > 5) trend = "faster";
    else if (diff < -5) trend = "slower";
    else trend = "similar";
  }

  return (
    <div className={styles.usage_item}>
      <div className={styles.usage_header}>
        <span className={styles.usage_label}>{label}</span>
        <span className={styles.usage_pct} title={t("account.tooltip_current")}>
          {pct}%
        </span>
      </div>
      <div className={styles.bar_track}>
        <div className={styles.bar_fill} style={{ width: `${Math.min(pct, 100)}%` }} />
        {prev !== null && (
          <div className={styles.bar_prev_marker} style={{ left: `${Math.min(prev, 100)}%` }} />
        )}
      </div>
      <div className={styles.usage_footer}>
        <span className={styles.usage_reset}>
          {t("account.resets_in", { t: formatResetIn(stats.resets_at, t) })}
        </span>
        {prev !== null && trend !== null && (
          <span
            className={`${styles.usage_prev} ${styles[`trend_${trend}`]}`}
            title={t("account.tooltip_prev", { n: prev, trend: t(`account.trend_${trend}`) })}
          >
            {trend === "faster" ? "\u2191" : trend === "slower" ? "\u2193" : "\u2248"} {prev}%
          </span>
        )}
      </div>
    </div>
  );
}

// ── Cursor usage bar (used / limit) ──────────────────────────────────────────

function CursorUsageBar({ item }: { item: CursorUsageItem }) {
  const { t } = useTranslation();
  // Prefer API-provided utilization; fall back to used/limit ratio
  const pct = item.utilization != null
    ? Math.round(item.utilization * 100)
    : item.limit
      ? Math.round((item.used / item.limit) * 100)
      : null;

  // Right-side label: show used/limit when both exist, otherwise percentage
  const rightLabel = item.limit != null && item.used != null
    ? `${item.used.toLocaleString()} / ${item.limit.toLocaleString()}`
    : pct != null
      ? `${pct}%`
      : null;

  return (
    <div className={styles.usage_item}>
      <div className={styles.usage_header}>
        <span className={styles.usage_label}>{item.name}</span>
        {rightLabel && <span className={styles.usage_pct}>{rightLabel}</span>}
      </div>
      {pct !== null && (
        <div className={styles.bar_track}>
          <div className={styles.bar_fill} style={{ width: `${Math.min(pct, 100)}%` }} />
        </div>
      )}
      {item.resetsAt && (
        <div className={styles.usage_footer}>
          <span className={styles.usage_reset}>
            {t("account.resets_in", { t: formatResetIn(item.resetsAt, t) })}
          </span>
        </div>
      )}
    </div>
  );
}

// ── Codex rate-limit window bar ──────────────────────────────────────────────

function formatWindowLabel(mins: number | null | undefined): string {
  if (!mins) return "";
  if (mins >= 1440) return `${Math.round(mins / 1440)}d`;
  if (mins >= 60) return `${Math.round(mins / 60)}h`;
  return `${mins}m`;
}

function CodexWindowBar({ label, window }: { label: string; window: CodexRateLimitWindow }) {
  const { t } = useTranslation();
  const pct = window.usedPercent;
  const resetIso = window.resetsAt
    ? new Date(window.resetsAt * 1000).toISOString()
    : null;

  return (
    <div className={styles.usage_item}>
      <div className={styles.usage_header}>
        <span className={styles.usage_label}>
          {label}
          {window.windowDurationMins ? ` (${formatWindowLabel(window.windowDurationMins)})` : ""}
        </span>
        <span className={styles.usage_pct}>{pct}%</span>
      </div>
      <div className={styles.bar_track}>
        <div className={styles.bar_fill} style={{ width: `${Math.min(pct, 100)}%` }} />
      </div>
      {resetIso && (
        <div className={styles.usage_footer}>
          <span className={styles.usage_reset}>
            {t("account.resets_in", { t: formatResetIn(resetIso, t) })}
          </span>
        </div>
      )}
    </div>
  );
}

// ── Section footer (shared) ──────────────────────────────────────────────────

function SectionFooter({
  lastUpdated,
  loading,
  autoRefresh,
  onAutoRefreshChange,
  onRefresh,
}: {
  lastUpdated: number | null;
  loading: boolean;
  autoRefresh: boolean;
  onAutoRefreshChange: (v: boolean) => void;
  onRefresh: () => void;
}) {
  const { t } = useTranslation();
  return (
    <div className={styles.tool_footer}>
      {lastUpdated && !loading && (
        <span className={styles.last_updated}>{formatLastUpdated(lastUpdated, t)}</span>
      )}
      <div className={styles.footer_actions}>
        <label className={styles.auto_toggle}>
          <input type="checkbox" checked={autoRefresh} onChange={(e) => onAutoRefreshChange(e.target.checked)} />
          {t("account.auto_5m")}
        </label>
        <button className={styles.refresh} onClick={onRefresh} disabled={loading} title={t("account.refresh_now")}>
          {"\u21BB"}
        </button>
      </div>
    </div>
  );
}

// ── Claude Code section ──────────────────────────────────────────────────────

function ClaudeUsageSection() {
  const { t } = useTranslation();
  const [info, setInfo] = useState<AccountInfoData | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [lastUpdated, setLastUpdated] = useState<number | null>(null);
  const [autoRefresh, setAutoRefresh] = useState(true);
  const [, setTick] = useState(0);
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null);

  async function load() {
    setLoading(true);
    setError(null);
    try {
      const data = await invoke<AccountInfoData>("get_account_info");
      setInfo(data);
      setLastUpdated(Date.now());
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => { load(); }, []);

  useEffect(() => {
    if (timerRef.current) clearInterval(timerRef.current);
    timerRef.current = autoRefresh ? setInterval(load, AUTO_REFRESH_INTERVAL_MS) : null;
    return () => { if (timerRef.current) clearInterval(timerRef.current); };
  }, [autoRefresh]);

  useEffect(() => {
    const timer = setInterval(() => setTick((n) => n + 1), 30_000);
    return () => clearInterval(timer);
  }, []);

  const hasUsage = info && (info.five_hour || info.seven_day || info.seven_day_sonnet);

  return (
    <div className={styles.tool_section}>
      <div className={styles.tool_header}>Claude Code</div>
      {loading && !info && <p className={styles.dim}>{t("account.loading")}</p>}
      {error && (
        <div className={styles.error}>
          <p>{error}</p>
          <button className={styles.retry} onClick={load}>{t("account.retry")}</button>
        </div>
      )}
      {hasUsage && (
        <div className={styles.bars}>
          <UsageBar label={t("account.five_hour")} stats={info.five_hour} />
          <UsageBar label={t("account.seven_day")} stats={info.seven_day} />
          <UsageBar label={t("account.seven_day_sonnet")} stats={info.seven_day_sonnet} />
        </div>
      )}
      {info && !hasUsage && <p className={styles.dim}>No usage data</p>}
      <SectionFooter
        lastUpdated={lastUpdated}
        loading={loading}
        autoRefresh={autoRefresh}
        onAutoRefreshChange={setAutoRefresh}
        onRefresh={load}
      />
    </div>
  );
}

// ── Cursor section ───────────────────────────────────────────────────────────

function CursorUsageSection() {
  const { t } = useTranslation();
  const [info, setInfo] = useState<CursorAccountInfoData | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [lastUpdated, setLastUpdated] = useState<number | null>(null);
  const [autoRefresh, setAutoRefresh] = useState(true);
  const [, setTick] = useState(0);
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null);

  async function load() {
    setLoading(true);
    setError(null);
    try {
      const data = await invoke<CursorAccountInfoData>("get_source_usage", { source: "cursor" });
      setInfo(data);
      setLastUpdated(Date.now());
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => { load(); }, []);

  useEffect(() => {
    if (timerRef.current) clearInterval(timerRef.current);
    timerRef.current = autoRefresh ? setInterval(load, AUTO_REFRESH_INTERVAL_MS) : null;
    return () => { if (timerRef.current) clearInterval(timerRef.current); };
  }, [autoRefresh]);

  useEffect(() => {
    const timer = setInterval(() => setTick((n) => n + 1), 30_000);
    return () => clearInterval(timer);
  }, []);

  return (
    <div className={styles.tool_section}>
      <div className={styles.tool_header}>Cursor</div>
      {loading && !info && <p className={styles.dim}>{t("account.loading")}</p>}
      {error && (
        <div className={styles.error}>
          <p>{error}</p>
          <button className={styles.retry} onClick={load}>{t("account.retry")}</button>
        </div>
      )}
      {info && info.usage.length > 0 && (
        <div className={styles.bars}>
          {info.usage.map((item) => (
            <CursorUsageBar key={item.name} item={item} />
          ))}
        </div>
      )}
      {info && info.usage.length === 0 && <p className={styles.dim}>No usage data</p>}
      <SectionFooter
        lastUpdated={lastUpdated}
        loading={loading}
        autoRefresh={autoRefresh}
        onAutoRefreshChange={setAutoRefresh}
        onRefresh={load}
      />
    </div>
  );
}

// ── Codex section ────────────────────────────────────────────────────────────

function CodexUsageSection() {
  const { t } = useTranslation();
  const [data, setData] = useState<CodexUsageItem | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [lastUpdated, setLastUpdated] = useState<number | null>(null);
  const [autoRefresh, setAutoRefresh] = useState(true);
  const [, setTick] = useState(0);
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null);

  async function load() {
    setLoading(true);
    setError(null);
    try {
      const result = await invoke<CodexUsageItem>("get_source_usage", { source: "codex" });
      setData(result);
      setLastUpdated(Date.now());
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => { load(); }, []);

  useEffect(() => {
    if (timerRef.current) clearInterval(timerRef.current);
    timerRef.current = autoRefresh ? setInterval(load, AUTO_REFRESH_INTERVAL_MS) : null;
    return () => { if (timerRef.current) clearInterval(timerRef.current); };
  }, [autoRefresh]);

  useEffect(() => {
    const timer = setInterval(() => setTick((n) => n + 1), 30_000);
    return () => clearInterval(timer);
  }, []);

  const hasBars = data && (data.primary || data.secondary);

  return (
    <div className={styles.tool_section}>
      <div className={styles.tool_header}>
        Codex
        {data?.planType && <span className={styles.plan_badge}>{data.planType}</span>}
      </div>
      {loading && !data && <p className={styles.dim}>{t("account.loading")}</p>}
      {error && (
        <div className={styles.error}>
          <p>{error}</p>
          <button className={styles.retry} onClick={load}>{t("account.retry")}</button>
        </div>
      )}
      {hasBars && (
        <div className={styles.bars}>
          {data.primary && (
            <CodexWindowBar label={t("account.five_hour")} window={data.primary} />
          )}
          {data.secondary && (
            <CodexWindowBar label={t("account.seven_day")} window={data.secondary} />
          )}
        </div>
      )}
      {data && !hasBars && <p className={styles.dim}>No usage data</p>}
      <SectionFooter
        lastUpdated={lastUpdated}
        loading={loading}
        autoRefresh={autoRefresh}
        onAutoRefreshChange={setAutoRefresh}
        onRefresh={load}
      />
    </div>
  );
}

// ── OpenClaw section ─────────────────────────────────────────────────────

interface OpenClawSessionUsage {
  sessionId: string;
  agentId: string;
  model: string;
  contextTokens: number;
  totalTokens: number | null;
  percentUsed: number | null;
  ageSecs: number;
}

interface OpenClawUsageInfo {
  sessions: OpenClawSessionUsage[];
}

function OpenClawContextBar({ session }: { session: OpenClawSessionUsage }) {
  const { t } = useTranslation();
  const pct = session.percentUsed != null
    ? Math.round(session.percentUsed)
    : session.totalTokens != null && session.contextTokens > 0
      ? Math.round((session.totalTokens / session.contextTokens) * 100)
      : null;

  const rightLabel = session.totalTokens != null
    ? `${(session.totalTokens / 1000).toFixed(0)}k / ${(session.contextTokens / 1000).toFixed(0)}k`
    : pct != null
      ? `${pct}%`
      : null;

  const ageMins = Math.round(session.ageSecs / 60);
  const ageLabel = ageMins < 60
    ? `${ageMins}m ago`
    : ageMins < 1440
      ? `${Math.round(ageMins / 60)}h ago`
      : `${Math.round(ageMins / 1440)}d ago`;

  return (
    <div className={styles.usage_item}>
      <div className={styles.usage_header}>
        <span className={styles.usage_label} title={`${session.agentId}/${session.sessionId.slice(0, 8)}`}>
          {session.model} ({ageLabel})
        </span>
        {rightLabel && <span className={styles.usage_pct}>{rightLabel}</span>}
      </div>
      {pct !== null && (
        <div className={styles.bar_track}>
          <div className={styles.bar_fill} style={{ width: `${Math.min(pct, 100)}%` }} />
        </div>
      )}
      <div className={styles.usage_footer}>
        <span className={styles.usage_reset}>{t("account.openclaw_context_window")}</span>
      </div>
    </div>
  );
}

function OpenClawUsageSection() {
  const { t } = useTranslation();
  const [data, setData] = useState<OpenClawUsageInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [lastUpdated, setLastUpdated] = useState<number | null>(null);
  const [autoRefresh, setAutoRefresh] = useState(true);
  const [, setTick] = useState(0);
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null);

  async function load() {
    setLoading(true);
    setError(null);
    try {
      const result = await invoke<OpenClawUsageInfo>("get_source_usage", { source: "openclaw" });
      setData(result);
      setLastUpdated(Date.now());
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => { load(); }, []);

  useEffect(() => {
    if (timerRef.current) clearInterval(timerRef.current);
    timerRef.current = autoRefresh ? setInterval(load, AUTO_REFRESH_INTERVAL_MS) : null;
    return () => { if (timerRef.current) clearInterval(timerRef.current); };
  }, [autoRefresh]);

  useEffect(() => {
    const timer = setInterval(() => setTick((n) => n + 1), 30_000);
    return () => clearInterval(timer);
  }, []);

  const hasSessions = data && data.sessions.length > 0;

  return (
    <div className={styles.tool_section}>
      <div className={styles.tool_header}>OpenClaw</div>
      {loading && !data && <p className={styles.dim}>{t("account.loading")}</p>}
      {error && (
        <div className={styles.error}>
          <p>{error}</p>
          <button className={styles.retry} onClick={load}>{t("account.retry")}</button>
        </div>
      )}
      {hasSessions && (
        <div className={styles.bars}>
          {data.sessions.map((s) => (
            <OpenClawContextBar key={s.sessionId} session={s} />
          ))}
        </div>
      )}
      {data && !hasSessions && <p className={styles.dim}>{t("account.openclaw_no_sessions")}</p>}
      <SectionFooter
        lastUpdated={lastUpdated}
        loading={loading}
        autoRefresh={autoRefresh}
        onAutoRefreshChange={setAutoRefresh}
        onRefresh={load}
      />
    </div>
  );
}

// ── Main panel ───────────────────────────────────────────────────────────────

interface DetectedTools {
  cli: boolean;
  vscode: boolean;
  jetbrains: boolean;
  desktop: boolean;
  cursor: boolean;
  openclaw: boolean;
  codex: boolean;
}

interface SetupStatus {
  detected_tools: DetectedTools;
  [key: string]: unknown;
}

export function UsagePanel() {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(true);
  const [hasClaude, setHasClaude] = useState(true);
  const [hasCursor, setHasCursor] = useState(false);
  const [hasCodex, setHasCodex] = useState(false);
  const [hasOpenclaw, setHasOpenclaw] = useState(false);

  useEffect(() => {
    invoke<SetupStatus>("check_setup_status")
      .then((s) => {
        const tools = s.detected_tools;
        setHasClaude(tools.cli || tools.vscode || tools.jetbrains || tools.desktop);
        setHasCursor(tools.cursor);
        setHasCodex(tools.codex);
        setHasOpenclaw(tools.openclaw);
      })
      .catch(() => {});
  }, []);

  if (!hasClaude && !hasCursor && !hasCodex && !hasOpenclaw) return null;

  return (
    <div className={styles.container}>
      <button className={styles.toggle} onClick={() => setExpanded((v) => !v)}>
        <span className={styles.toggle_label}>{t("account.usage")}</span>
        <span className={styles.toggle_icon}>{expanded ? "\u25B2" : "\u25BC"}</span>
      </button>
      {expanded && (
        <div className={styles.content}>
          {hasClaude && <ClaudeUsageSection />}
          {hasOpenclaw && <OpenClawUsageSection />}
          {hasCursor && <CursorUsageSection />}
          {hasCodex && <CodexUsageSection />}
        </div>
      )}
    </div>
  );
}
