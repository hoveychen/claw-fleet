import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
import styles from "./AccountInfo.module.css";

const AUTO_REFRESH_INTERVAL_MS = 1 * 60 * 1000; // 1 minute

interface UsageStats {
  utilization: number; // 0–100
  resets_at: string;
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

function formatResetIn(resets_at: string): string {
  const diff = new Date(resets_at).getTime() - Date.now();
  if (diff <= 0) return "soon";
  const h = Math.floor(diff / 3600000);
  const d = Math.floor(h / 24);
  if (d >= 1) return `${d}d`;
  if (h >= 1) return `${h}h`;
  const m = Math.floor(diff / 60000);
  return `${m}m`;
}

function formatLastUpdated(ts: number | null): string {
  if (!ts) return "";
  const diff = Date.now() - ts;
  if (diff < 5000) return "just now";
  const m = Math.floor(diff / 60000);
  if (m < 1) return `${Math.floor(diff / 1000)}s ago`;
  return `${m}m ago`;
}

function UsageBar({
  label,
  stats,
}: {
  label: string;
  stats: UsageStats | null;
}) {
  if (!stats) return null;
  const pct = Math.round(stats.utilization);
  return (
    <div className={styles.usage_item}>
      <div className={styles.usage_header}>
        <span className={styles.usage_label}>{label}</span>
        <span className={styles.usage_pct}>{pct}%</span>
      </div>
      <div className={styles.bar_track}>
        <div
          className={styles.bar_fill}
          style={{ width: `${Math.min(pct, 100)}%` }}
        />
      </div>
      <div className={styles.usage_reset}>
        Resets in {formatResetIn(stats.resets_at)}
      </div>
    </div>
  );
}

export function AccountInfo() {
  const [info, setInfo] = useState<AccountInfoData | null>(null);
  const [error, setError] = useState<string | null>(null);
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
    const t = setInterval(() => setTick((n) => n + 1), 30_000);
    return () => clearInterval(t);
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
        <span className={styles.toggle_label}>Account & Usage</span>
        <span className={styles.toggle_icon}>{expanded ? "▲" : "▼"}</span>
      </button>

      {expanded && (
        <div className={styles.panel}>
          {loading && <p className={styles.dim}>Loading…</p>}
          {error && (
            <p className={styles.error}>
              {error}
              <button className={styles.retry} onClick={load}>
                Retry
              </button>
            </p>
          )}
          {info && (
            <>
              <section className={styles.section}>
                <div className={styles.section_title}>Account</div>
                <Row label="Auth method" value="Claude AI" />
                <Row label="Email" value={info.email} />
                <Row label="Organization" value={info.organization_name} />
                <Row label="Plan" value={info.plan} />
              </section>

              <section className={styles.section}>
                <div className={styles.section_title}>Usage</div>
                <UsageBar label="Session (5hr)" stats={info.five_hour} />
                <UsageBar label="Weekly (7 day)" stats={info.seven_day} />
                <UsageBar
                  label="Weekly Sonnet"
                  stats={info.seven_day_sonnet}
                />
              </section>
            </>
          )}

          <div className={styles.footer}>
            {lastUpdated && !loading && (
              <span className={styles.last_updated}>
                Updated {formatLastUpdated(lastUpdated)}
              </span>
            )}
            <div className={styles.footer_actions}>
              <label className={styles.auto_toggle}>
                <input
                  type="checkbox"
                  checked={autoRefresh}
                  onChange={(e) => setAutoRefresh(e.target.checked)}
                />
                Auto (1m)
              </label>
              <button
                className={styles.refresh}
                onClick={load}
                disabled={loading}
                title="Refresh now"
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
