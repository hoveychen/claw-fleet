import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import styles from "./AccountInfo.module.css";

const AUTO_REFRESH_INTERVAL_MS = 5 * 60 * 1000; // 5 minutes

interface UsageStats {
  utilization: number; // 0–100
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

function UsageBar({
  label,
  stats,
}: {
  label: string;
  stats: UsageStats | null;
}) {
  const { t } = useTranslation();
  if (!stats) return null;
  const pct = Math.round(stats.utilization);
  const prev =
    stats.prev_utilization !== null && stats.prev_utilization !== undefined
      ? Math.round(stats.prev_utilization)
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
        <span
          className={styles.usage_pct}
          title={t("account.tooltip_current")}
        >
          {pct}%
        </span>
      </div>
      <div className={styles.bar_track}>
        <div
          className={styles.bar_fill}
          style={{ width: `${Math.min(pct, 100)}%` }}
        />
        {prev !== null && (
          <div
            className={styles.bar_prev_marker}
            style={{ left: `${Math.min(prev, 100)}%` }}
          />
        )}
      </div>
      <div className={styles.usage_footer}>
        <span className={styles.usage_reset}>
          {t("account.resets_in", { t: formatResetIn(stats.resets_at, t) })}
        </span>
        {prev !== null && trend !== null && (
          <span
            className={`${styles.usage_prev} ${styles[`trend_${trend}`]}`}
            title={t("account.tooltip_prev", {
              n: prev,
              trend: t(`account.trend_${trend}`),
            })}
          >
            {trend === "faster" ? "↑" : trend === "slower" ? "↓" : "≈"}{" "}
            {prev}%
          </span>
        )}
      </div>
    </div>
  );
}

export function AccountInfo() {
  const { t } = useTranslation();
  const [info, setInfo] = useState<AccountInfoData | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [logPath, setLogPath] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [expanded, setExpanded] = useState(true);
  const [autoRefresh, setAutoRefresh] = useState(true);
  const [lastUpdated, setLastUpdated] = useState<number | null>(null);
  const [, setTick] = useState(0); // force re-render for "Xm ago" display
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
      if (!logPath) {
        invoke<string>("get_log_path").then(setLogPath).catch(() => {});
      }
    } finally {
      setLoading(false);
    }
  }

  // Initial load
  useEffect(() => {
    load();
  }, []);

  // Auto-refresh timer
  useEffect(() => {
    if (timerRef.current) {
      clearInterval(timerRef.current);
      timerRef.current = null;
    }
    if (autoRefresh) {
      timerRef.current = setInterval(load, AUTO_REFRESH_INTERVAL_MS);
    }
    return () => {
      if (timerRef.current) clearInterval(timerRef.current);
    };
  }, [autoRefresh]);

  // Tick every 30s to keep "Xm ago" fresh
  useEffect(() => {
    const timer = setInterval(() => setTick((n) => n + 1), 30_000);
    return () => clearInterval(timer);
  }, []);

  return (
    <div className={styles.container}>
      <button
        className={styles.toggle}
        onClick={() => {
          setExpanded((v) => !v);
          if (!expanded && !info && !loading) load();
        }}
      >
        <span className={styles.toggle_label}>{t("account.panel_title")}</span>
        <span className={styles.toggle_icon}>{expanded ? "▲" : "▼"}</span>
      </button>

      {expanded && (
        <div className={styles.panel}>
          {loading && <p className={styles.dim}>{t("account.loading")}</p>}
          {error && (
            <div className={styles.error}>
              <p>{error}</p>
              {logPath && (
                <p className={styles.log_hint}>
                  {t("account.debug_log", { path: logPath })}
                </p>
              )}
              <button className={styles.retry} onClick={load}>
                {t("account.retry")}
              </button>
            </div>
          )}
          {info && (
            <>
              <section className={styles.section}>
                <div className={styles.section_title}>{t("account.title")}</div>
                <Row label={t("account.auth")} value="Claude AI" />
                <Row label={t("account.email")} value={info.email} />
                <Row label={t("account.org")} value={info.organization_name} />
                <Row label={t("account.plan")} value={info.plan} />
              </section>

              <section className={styles.section}>
                <div className={styles.section_title}>{t("account.usage")}</div>
                <UsageBar label={t("account.five_hour")} stats={info.five_hour} />
                <UsageBar label={t("account.seven_day")} stats={info.seven_day} />
                <UsageBar
                  label={t("account.seven_day_sonnet")}
                  stats={info.seven_day_sonnet}
                />
              </section>
            </>
          )}

          <div className={styles.footer}>
            {lastUpdated && !loading && (
              <span className={styles.last_updated}>
                {formatLastUpdated(lastUpdated, t)}
              </span>
            )}
            <div className={styles.footer_actions}>
              <label className={styles.auto_toggle}>
                <input
                  type="checkbox"
                  checked={autoRefresh}
                  onChange={(e) => setAutoRefresh(e.target.checked)}
                />
                {t("account.auto_5m")}
              </label>
              <button
                className={styles.refresh}
                onClick={load}
                disabled={loading}
                title={t("account.refresh_now")}
              >
                ↻
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className={styles.row}>
      <span className={styles.row_label}>{label}</span>
      <span className={styles.row_value}>{value}</span>
    </div>
  );
}
