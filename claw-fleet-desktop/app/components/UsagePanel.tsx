import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { ClaudeIcon, CodexIcon, DshIcon, FoxyIcon } from "./SessionCard";
import styles from "./UsagePanel.module.css";
import {
  useUsageStore,
  type UsageStats,
  type CodexRateLimitWindow,
  type DshProviderBalance,
} from "../usageStore";
import type { SourceInfo } from "../modelChoices";
import { codexRateLimitBars, type TFunc } from "../codexUsage";
import { useUsageRing } from "../hooks/useUsageRing";
import { UsageHistoryModal } from "./UsageHistoryModal";
import { CodexUsageHistoryModal } from "./CodexUsageHistoryModal";

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

function fillClass(pct: number): string {
  if (pct >= 85) return styles.bar_fill_critical;
  if (pct >= 60) return styles.bar_fill_warn;
  return styles.bar_fill;
}

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
        <div className={fillClass(pct)} style={{ width: `${Math.min(pct, 100)}%` }} />
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


// ── Codex rate-limit window bar ──────────────────────────────────────────────

// Codex reports each rate-limit window's real length via `windowDurationMins`
// (unlike Claude's fixed 5h + 7d pools). So the label must be *derived* from
// that duration, not from the primary/secondary slot — a Team plan, for
// example, returns a single 7-day window in the `primary` slot, and hardcoding
// "会话 (5小时)" there produced the self-contradicting "会话 (5小时) (7d)".
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
        </span>
        <span className={styles.usage_pct}>{pct}%</span>
      </div>
      <div className={styles.bar_track}>
        <div className={fillClass(pct)} style={{ width: `${Math.min(pct, 100)}%` }} />
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

// ── dsh money row ────────────────────────────────────────────────────────────

/** Render an amount in the currency the provider reported it in.
 *
 *  The currency is carried per row rather than assumed, because the two
 *  providers behind a dsh install do not agree: DeepSeek settles a top-up in
 *  CNY, OpenRouter in USD. Printing one of them with the other's sign is the
 *  kind of confident-wrong number this panel exists to avoid, so an unknown
 *  code is prefixed verbatim instead of being guessed at. */
function formatMoney(amount: number, currency: string | null | undefined): string {
  const n = amount.toFixed(2);
  if (currency === "CNY") return `¥${n}`;
  if (currency === "USD") return `$${n}`;
  return currency ? `${currency} ${n}` : n;
}

/** One provider's position: a bar only when the provider gave a denominator.
 *
 *  A balance is money left with nothing to divide by — a bar drawn from it
 *  would have to invent a ceiling. OpenRouter's per-key `limit` is a real
 *  denominator, so that row (and only that row) gets the same bar treatment as
 *  the rate-limit sections above. */
function DshBalanceRow({ balance }: { balance: DshProviderBalance }) {
  const { t } = useTranslation();
  const hasLimit =
    balance.limit !== null &&
    balance.limit !== undefined &&
    balance.limit > 0 &&
    balance.used !== null &&
    balance.used !== undefined;
  const pct = hasLimit ? Math.round((balance.used! / balance.limit!) * 100) : null;

  return (
    <div className={styles.usage_item}>
      <div className={styles.usage_header}>
        <span className={styles.usage_label}>{balance.label}</span>
        {balance.balance !== null && balance.balance !== undefined && (
          <span className={styles.usage_pct} title={t("account.dsh_balance_tip")}>
            {formatMoney(balance.balance, balance.currency)}
          </span>
        )}
      </div>
      {pct !== null && (
        <>
          <div className={styles.bar_track}>
            <div className={fillClass(pct)} style={{ width: `${Math.min(pct, 100)}%` }} />
          </div>
          <div className={styles.usage_footer}>
            <span className={styles.usage_reset}>
              {t("account.dsh_key_limit", {
                used: formatMoney(balance.used!, balance.currency),
                limit: formatMoney(balance.limit!, balance.currency),
              })}
            </span>
          </div>
        </>
      )}
      {balance.error && <div className={styles.usage_footer}>
        <span className={styles.usage_reset}>{balance.error}</span>
      </div>}
    </div>
  );
}

// ── Usage-source mark (shared) ───────────────────────────────────────────────

/** Foxy's fox head, shown on a card whose numbers came from the local
 *  foxy-switcher daemon. An icon rather than the old "Foxy Switcher" text
 *  badge: the header is one flex line shared with the plan badge, and two text
 *  badges pushed the provider mark out of the Codex card entirely. The other
 *  source ("anthropic" / "codex-app-server") is the provider itself, which the
 *  header's own mark and title already say — so it renders nothing. */
function UsageSourceMark({ source }: { source: string | null | undefined }) {
  const { t } = useTranslation();
  if (source !== "foxy-switcher") return null;
  return (
    <span
      className={styles.source_icon}
      title={`${t("account.usage_source")}: ${t("account.usage_source_foxy")}`}
    >
      <FoxyIcon />
    </span>
  );
}

// ── Section footer (shared) ──────────────────────────────────────────────────

function SectionFooter({
  lastUpdated,
  loading,
  autoRefresh,
  onAutoRefreshChange,
  onRefresh,
  hideAutoToggle,
}: {
  lastUpdated: number | null;
  loading: boolean;
  autoRefresh?: boolean;
  onAutoRefreshChange?: (v: boolean) => void;
  onRefresh: () => void;
  // When true the per-section auto-refresh checkbox is omitted; the section's
  // cadence is managed elsewhere (currently only Claude, where the store
  // adapts the interval to usage_source \u2014 see armClaudeTimer in usageStore).
  hideAutoToggle?: boolean;
}) {
  const { t } = useTranslation();
  return (
    <div className={styles.tool_footer}>
      {lastUpdated && !loading && (
        <span className={styles.last_updated}>{formatLastUpdated(lastUpdated, t)}</span>
      )}
      <div className={styles.footer_actions}>
        {!hideAutoToggle && onAutoRefreshChange && (
          <label className={styles.auto_toggle}>
            <input
              type="checkbox"
              checked={autoRefresh ?? false}
              onChange={(e) => onAutoRefreshChange(e.target.checked)}
            />
            {t("account.auto_5m")}
          </label>
        )}
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
  const { data: info, error, loading, lastUpdated } = useUsageStore((s) => s.claude);
  const load = useUsageStore((s) => s.load);
  const [, setTick] = useState(0);
  const [historyOpen, setHistoryOpen] = useState(false);

  useEffect(() => {
    const timer = setInterval(() => setTick((n) => n + 1), 30_000);
    return () => clearInterval(timer);
  }, []);

  const refresh = () => { load("claude"); };

  const scoped = info?.seven_day_scoped ?? [];
  const hasUsage = info && (info.five_hour || info.seven_day || scoped.length > 0);

  return (
    <div className={styles.tool_section}>
      <div className={styles.tool_header}>
        <ClaudeIcon />
        <span className={styles.tool_name}>Claude Code</span>
        {info?.plan && (
          <span className={styles.plan_badge} title={info.plan}>{info.plan}</span>
        )}
        <UsageSourceMark source={info?.usage_source} />
      </div>
      {info?.email && (
        <div className={styles.account_line} title={t("account.email")}>
          {info.email}
        </div>
      )}
      {loading && !info && <p className={styles.dim}>{t("account.loading")}</p>}
      {error && (
        <div className={styles.error}>
          <p>{error}</p>
          <button className={styles.retry} onClick={refresh}>{t("account.retry")}</button>
        </div>
      )}
      {hasUsage && (
        <div className={styles.bars}>
          <UsageBar label={t("account.five_hour")} stats={info.five_hour} />
          <UsageBar label={t("account.seven_day")} stats={info.seven_day} />
          {scoped.map((sc) => (
            <UsageBar
              key={sc.model_label}
              label={t("account.seven_day_scoped", { model: sc.model_label })}
              stats={sc}
            />
          ))}
        </div>
      )}
      {info && !hasUsage && (
        <p className={styles.dim}>{t("account.no_usage_data", "暂无用量数据")}</p>
      )}
      {hasUsage && (
        <button
          className={styles.history_btn}
          onClick={() => setHistoryOpen(true)}
          title={t("account.occupancy_subtitle")}
        >
          {t("account.occupancy_history")}
        </button>
      )}
      {historyOpen && <UsageHistoryModal onClose={() => setHistoryOpen(false)} />}
      <SectionFooter
        lastUpdated={lastUpdated}
        loading={loading}
        onRefresh={refresh}
        hideAutoToggle
      />
    </div>
  );
}

// ── Codex section ────────────────────────────────────────────────────────────

function CodexUsageSection() {
  const { t } = useTranslation();
  const { data, error, loading, lastUpdated, autoRefresh } = useUsageStore((s) => s.codex);
  const load = useUsageStore((s) => s.load);
  const setAutoRefresh = useUsageStore((s) => s.setAutoRefresh);
  const [, setTick] = useState(0);

  useEffect(() => {
    const timer = setInterval(() => setTick((n) => n + 1), 30_000);
    return () => clearInterval(timer);
  }, []);

  const [historyOpen, setHistoryOpen] = useState(false);

  const refresh = () => { load("codex"); };
  const onAutoRefreshChange = (v: boolean) => setAutoRefresh("codex", v);

  const bars = data ? codexRateLimitBars(data, t) : [];
  const hasBars = bars.length > 0;

  return (
    <div className={styles.tool_section}>
      <div className={styles.tool_header}>
        <CodexIcon />
        <span className={styles.tool_name}>Codex</span>
        {data?.planType && (
          <span className={styles.plan_badge} title={data.planType}>{data.planType}</span>
        )}
        <UsageSourceMark source={data?.usageSource} />
      </div>
      {data?.email && (
        <div className={styles.account_line} title={t("account.email")}>
          {data.email}
        </div>
      )}
      {loading && !data && <p className={styles.dim}>{t("account.loading")}</p>}
      {error && (
        <div className={styles.error}>
          <p>{error}</p>
          <button className={styles.retry} onClick={refresh}>{t("account.retry")}</button>
        </div>
      )}
      {hasBars && (
        <div className={styles.bars}>
          {bars.map((bar) => (
            <CodexWindowBar key={bar.key} label={bar.label} window={bar.window} />
          ))}
        </div>
      )}
      {data && !hasBars && (
        <p className={styles.dim}>{t("account.no_usage_data", "暂无用量数据")}</p>
      )}
      {hasBars && (
        <button
          className={styles.history_btn}
          onClick={() => setHistoryOpen(true)}
          title={t("account.occupancy_subtitle_codex")}
        >
          {t("account.occupancy_history")}
        </button>
      )}
      {historyOpen && <CodexUsageHistoryModal onClose={() => setHistoryOpen(false)} />}
      <SectionFooter
        lastUpdated={lastUpdated}
        loading={loading}
        autoRefresh={autoRefresh}
        onAutoRefreshChange={onAutoRefreshChange}
        onRefresh={refresh}
      />
    </div>
  );
}

// ── dsh section ──────────────────────────────────────────────────────────────

/** dsh's card reports **money**, not a rate-limit window.
 *
 *  dsh publishes no quota of its own — it is a bring-your-own-key harness, so
 *  the only truthful number is what the provider behind the key says is left.
 *  That is why this section has no plan badge, no reset countdown and no
 *  occupancy-history button: none of those exist for a prepaid balance. */
function DshUsageSection() {
  const { t } = useTranslation();
  const { data, error, loading, lastUpdated, autoRefresh } = useUsageStore((s) => s.dsh);
  const load = useUsageStore((s) => s.load);
  const setAutoRefresh = useUsageStore((s) => s.setAutoRefresh);
  const [, setTick] = useState(0);

  useEffect(() => {
    const timer = setInterval(() => setTick((n) => n + 1), 30_000);
    return () => clearInterval(timer);
  }, []);

  const refresh = () => { load("dsh"); };
  const onAutoRefreshChange = (v: boolean) => setAutoRefresh("dsh", v);

  const balances = data?.balances ?? [];

  return (
    <div className={styles.tool_section}>
      <div className={styles.tool_header}>
        <DshIcon />
        <span className={styles.tool_name}>dsh</span>
      </div>
      {loading && !data && <p className={styles.dim}>{t("account.loading")}</p>}
      {error && (
        <div className={styles.error}>
          <p>{error}</p>
          <button className={styles.retry} onClick={refresh}>{t("account.retry")}</button>
        </div>
      )}
      {balances.length > 0 && (
        <div className={styles.bars}>
          {balances.map((b) => (
            <DshBalanceRow key={b.provider} balance={b} />
          ))}
        </div>
      )}
      {data && balances.length === 0 && (
        <p className={styles.dim}>{t("account.dsh_no_keys")}</p>
      )}
      <SectionFooter
        lastUpdated={lastUpdated}
        loading={loading}
        autoRefresh={autoRefresh}
        onAutoRefreshChange={onAutoRefreshChange}
        onRefresh={refresh}
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
  codex: boolean;
}

interface SetupStatus {
  detected_tools: DetectedTools;
  [key: string]: unknown;
}

export function UsagePanel({ collapsed = false }: { collapsed?: boolean } = {}) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(true);
  const [hasClaude, setHasClaude] = useState(true);
  const [hasCodex, setHasCodex] = useState(false);
  // dsh is not in `detected_tools` (that struct predates it and describes
  // Claude's install surfaces plus codex). Its own registry entry already
  // answers the question — `available` is gated on the binary existing and
  // `enabled` on the settings toggle — so the section follows the same rule
  // the launcher uses rather than growing a second detection path.
  const [hasDsh, setHasDsh] = useState(false);
  const ring = useUsageRing();
  // Auto-load Claude usage when collapsed (so tile has data without expanding panel)
  const loadUsage = useUsageStore((s) => s.load);
  useEffect(() => {
    if (!collapsed) return;
    if (hasClaude) loadUsage("claude");
    if (hasCodex) loadUsage("codex");
    if (hasDsh) loadUsage("dsh");
  }, [collapsed, hasClaude, hasCodex, hasDsh, loadUsage]);

  useEffect(() => {
    invoke<SetupStatus>("check_setup_status")
      .then((s) => {
        const tools = s.detected_tools;
        setHasClaude(tools.cli || tools.vscode || tools.jetbrains || tools.desktop);
        setHasCodex(tools.codex);
      })
      .catch(() => {});
    invoke<SourceInfo[]>("get_sources_config")
      .then((sources) => {
        setHasDsh(sources.some((s) => s.name === "dsh" && s.enabled && s.available));
      })
      .catch(() => {});
  }, []);

  if (!hasClaude && !hasCodex && !hasDsh) return null;

  if (collapsed) {
    if (!ring) {
      return (
        <div className={styles.tile} title={t("account.loading")}>
          <span className={styles.tile_value}>—</span>
          <span className={styles.tile_label}>{t("account.usage")}</span>
        </div>
      );
    }
    const detail = ring.sources
      .map((s) => `${s.name}: ${Math.round(s.percent)}%`)
      .join("\n");
    const tooltip = `${t("account.usage")} — ${ring.topSource} ${Math.round(ring.overall)}%\n${detail}`;
    return (
      <div className={styles.tile} title={tooltip}>
        <span className={styles.tile_value}>{Math.round(ring.overall)}%</span>
        <span className={styles.tile_label}>{ring.topSource}</span>
      </div>
    );
  }

  return (
    <div className={styles.container}>
      <button className={styles.toggle} onClick={() => setExpanded((v) => !v)}>
        <span className={styles.toggle_label}>{t("account.usage")}</span>
        <span className={styles.toggle_icon}>{expanded ? "\u25B2" : "\u25BC"}</span>
      </button>
      {expanded && (
        <div className={styles.content}>
          {hasClaude && <ClaudeUsageSection />}
          {hasCodex && <CodexUsageSection />}
          {hasDsh && <DshUsageSection />}
        </div>
      )}
    </div>
  );
}
