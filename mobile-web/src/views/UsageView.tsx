// "Account & Usage" subpage: bringing together desktop's AccountInfo and UsagePanel
// onto mobile — today's cumulative spend (reusing today_usage already polled by App),
// Claude account profile and 5h/7d rate limit bars, plus normalized usage bars from
// other agent sources (codex). Data comes through relay's `account_usage` (see
// ../account.ts).

import { useCallback, useEffect, useState } from "react";
import { RefreshCw } from "lucide-react";
import { fetchAccountUsage } from "../account";
import { t } from "../i18n";
import type { FleetTransport } from "../transport";
import type { AccountUsage, TodayUsage, UsageBar } from "../types";
import { FoxyIcon } from "./AgentSourceIcon";
import { UsageChart } from "./UsageChart";
import { CodexUsageChart } from "./CodexUsageChart";
import styles from "./UsageView.module.css";
import { AppHeader } from "./AppHeader";
import { HeaderAction } from "./HeaderAction";

/** How much one device spent in "Today's Cumulative". `usage` is `null` = this device
 *  hasn't reported yet. */
export interface DeviceUsageRow {
  id: string;
  label: string;
  usage: TodayUsage | null;
}

interface Props {
  client: FleetTransport | null;
  /** Today's cumulative from App header, reused directly — avoid re-scanning sessions
   *  for the same number. */
  todayUsage: TodayUsage | null;
  /** Per-device breakdown totals. Empty array when only one device is configured —
   *  the breakdown is the total itself, and extra lines are just noise. */
  perDevice?: DeviceUsageRow[];
  /** Name of the current scope's device, to annotate "these numbers are for this
   *  device only". `null` when only one device — no other devices to confuse with,
   *  and adding a label is just noise. */
  activeDeviceLabel?: string | null;
  onBack: () => void;
}

/** Section subheading. When `device` is provided, append "from <device name>" on the
 *  right — "Today's Cumulative" is the sum of all devices, while the blocks below
 *  (account, rate limits, charts) read from **the current device only**. When two
 *  devices are logged into different accounts, this distinction determines whose limits
 *  the bars refer to. */
function SectionHead({ label, device }: { label: string; device?: string | null }) {
  return (
    <div className={styles.sectionLabel}>
      <span>{label}</span>
      {device && <span className={styles.sectionDevice}>{t("来自 {0}", device)}</span>}
    </div>
  );
}

/** Display names for each source in section headings; unknown sources fall back to raw
 *  id. */
const SOURCE_LABEL: Record<string, string> = {
  codex: "Codex",
  dsh: "DeepSeek Harness",
};

/** Format amounts by the currency each provider reports. They differ — DeepSeek
 *  settles in CNY, OpenRouter in USD — so currency follows each transaction. Unknown
 *  currencies get their code as-is, no guessing. */
function fmtMoney(amount: number, currency: string | null): string {
  const n = amount.toFixed(2);
  if (currency === "CNY") return `¥${n}`;
  if (currency === "USD") return `$${n}`;
  return currency ? `${currency} ${n}` : n;
}

/** Compact token count: 1.2M / 34.5K / 780. */
function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return `${n}`;
}

/** How much time until the rate limit window resets. Returns null if expired or the
 *  timestamp can't parse. */
function formatResetIn(resetsAt: string | null | undefined): string | null {
  if (!resetsAt) return null;
  const ms = new Date(resetsAt).getTime();
  if (Number.isNaN(ms)) return null;
  const diff = ms - Date.now();
  if (diff <= 0) return t("即将重置");
  const h = Math.floor(diff / 3_600_000);
  const d = Math.floor(h / 24);
  if (d >= 1) return t("{0} 天后重置", d);
  if (h >= 1) return t("{0} 小时后重置", h);
  return t("{0} 分钟后重置", Math.max(1, Math.floor(diff / 60_000)));
}

/** Same thresholds as desktop UsagePanel: warning tone at 60%, critical at 85%. */
function tone(pct: number): "ok" | "warn" | "critical" {
  if (pct >= 85) return "critical";
  if (pct >= 60) return "warn";
  return "ok";
}

function Bar({ bar }: { bar: UsageBar }) {
  const pct = Math.round(bar.utilization * 100);
  const prev =
    bar.prevUtilization === null || bar.prevUtilization === undefined
      ? null
      : Math.round(bar.prevUtilization * 100);
  const resetIn = formatResetIn(bar.resetsAt);

  return (
    <div className={styles.bar}>
      <div className={styles.barHead}>
        <span className={styles.barLabel}>{bar.label}</span>
        <span className={styles.barPct} data-tone={tone(pct)}>
          {pct}%
        </span>
      </div>
      <div className={styles.barTrack}>
        <div
          className={styles.barFill}
          data-tone={tone(pct)}
          style={{ width: `${Math.min(Math.max(pct, 0), 100)}%` }}
        />
        {prev !== null && (
          <div
            className={styles.barPrev}
            style={{ left: `${Math.min(Math.max(prev, 0), 100)}%` }}
          />
        )}
      </div>
      {(resetIn || prev !== null) && (
        <div className={styles.barFoot}>
          <span>{resetIn}</span>
          {prev !== null && (
            <span className={styles.barPrevLabel}>{t("上一周期 {0}%", prev)}</span>
          )}
        </div>
      )}
    </div>
  );
}

function Row({ label, value }: { label: string; value: React.ReactNode }) {
  return (
    <div className={styles.row}>
      <span className={styles.rowLabel}>{label}</span>
      <span className={styles.rowValue}>{value}</span>
    </div>
  );
}

/** Value for the "Usage Source" row: foxy draws a fox head, the same mark as desktop
 *  card headers. Non-foxy stays as text (each provider's own channel name) — no hover
 *  tooltip on mobile, and an OpenAI/Anthropic mark here would duplicate the source name
 *  in the title, making it harder to read what the source is. */
function UsageSourceValue({
  source,
  fallback,
}: {
  source: string | null | undefined;
  fallback: string;
}) {
  if (source !== "foxy-switcher") return <>{fallback}</>;
  return (
    <span className={styles.sourceIcon} aria-label="foxy-switcher">
      <FoxyIcon />
    </span>
  );
}

export function UsageView({
  client,
  todayUsage,
  perDevice = [],
  activeDeviceLabel = null,
  onBack,
}: Props) {
  const [data, setData] = useState<AccountUsage | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  // Increment on refresh, use as the chart component's key — forces it to re-fetch
  // along with the account data.
  const [reloadKey, setReloadKey] = useState(0);

  const refresh = useCallback(async () => {
    if (!client) return;
    setLoading(true);
    setError(null);
    setReloadKey((n) => n + 1);
    try {
      setData(await fetchAccountUsage(client));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, [client]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const claude = data?.claude ?? null;

  return (
    <div className={styles.page}>
      <AppHeader
        onBack={onBack}
        title={t("账号与用量")}
        actions={
          <HeaderAction
            icon={<RefreshCw size={17} />}
            label={t("刷新")}
            onClick={() => void refresh()}
            busy={loading}
            disabled={loading}
          />
        }
      />

      <div className={styles.body}>
        {/* ── Today's Cumulative ── */}
        <div className={styles.section}>
          <div className={styles.sectionLabel}>{t("今日累计")}</div>
          <div className={styles.card}>
            {todayUsage ? (
              <>
                <div className={styles.today}>
                  <span className={styles.todayCost}>${todayUsage.costUsd.toFixed(2)}</span>
                  <span className={styles.todayTokens}>
                    {t("{0} token", fmtTokens(todayUsage.inputTokens + todayUsage.outputTokens))}
                  </span>
                </div>
                <div className={styles.divider} />
                <Row
                  label={t("会话花费")}
                  value={`$${todayUsage.agentCostUsd.toFixed(2)} · ${t("{0} 个会话", todayUsage.sessionCount)}`}
                />
                <Row label={t("Fleet 自身花费")} value={`$${todayUsage.fleetCostUsd.toFixed(2)}`} />
                {/* The total is **all** devices combined (deviceRuntime's totalUsage), but
                    it can't answer "which device is spending". Two devices might be logged
                    into different accounts. So with multiple devices, break it back into
                    one row per device. */}
                {perDevice.length > 1 && (
                  <>
                    <div className={styles.divider} />
                    {perDevice.map((d) => (
                      <Row
                        key={d.id}
                        label={d.label}
                        value={
                          d.usage ? (
                            `$${d.usage.costUsd.toFixed(2)} · ${fmtTokens(
                              d.usage.inputTokens + d.usage.outputTokens,
                            )}`
                          ) : (
                            <span className={styles.rowMuted}>{t("未上报")}</span>
                          )
                        }
                      />
                    ))}
                  </>
                )}
              </>
            ) : (
              <div className={styles.hint}>{t("桌面端离线，拿不到今日用量。")}</div>
            )}
          </div>
        </div>

        {error && <div className={styles.hint}>{t("用量加载失败：{0}", error)}</div>}
        {!error && !data && loading && <div className={styles.hint}>{t("加载中…")}</div>}

        {/* ── Claude Account ── */}
        {data && (
          <div className={styles.section}>
            <SectionHead label="Claude Code" device={activeDeviceLabel} />
            <div className={styles.card}>
              {claude ? (
                <>
                  {claude.email && <Row label={t("账号")} value={claude.email} />}
                  {claude.organizationName && (
                    <Row label={t("组织")} value={claude.organizationName} />
                  )}
                  {claude.plan && <Row label={t("套餐")} value={claude.plan} />}
                  <Row
                    label={t("用量来源")}
                    value={
                      <UsageSourceValue
                        source={claude.usageSource}
                        fallback={t("Anthropic 接口")}
                      />
                    }
                  />
                  {claude.bars.length > 0 && (
                    <>
                      <div className={styles.divider} />
                      <div className={styles.bars}>
                        {claude.bars.map((b) => (
                          <Bar key={b.label} bar={b} />
                        ))}
                      </div>
                    </>
                  )}
                  {claude.bars.length === 0 && (
                    <div className={styles.hint}>{t("这个账号没有限流数据。")}</div>
                  )}
                </>
              ) : (
                <div className={styles.hint}>
                  {t("Claude 账号读取失败：{0}", data.claudeError ?? t("未知原因"))}
                </div>
              )}
            </div>
          </div>
        )}

        {/* ── Utilization curve · last 24 hours ── */}
        <div className={styles.section}>
          <SectionHead label={t("占用率变化 · 近 24 小时")} device={activeDeviceLabel} />
          <div className={styles.card}>
            <UsageChart key={reloadKey} client={client} />
          </div>
        </div>

        {/* ── Other agent sources ── */}
        {data?.sources.map((s) => (
          <div key={s.source} className={styles.section}>
            <SectionHead
              label={SOURCE_LABEL[s.source] ?? s.source}
              device={activeDeviceLabel}
            />
            <div className={styles.card}>
              {s.email && <Row label={t("账号")} value={s.email} />}
              {s.plan && <Row label={t("套餐")} value={s.plan} />}
              {s.usageSource && (
                <Row
                  label={t("用量来源")}
                  value={
                    <UsageSourceValue
                      source={s.usageSource}
                      fallback={t("Codex app-server")}
                    />
                  }
                />
              )}
              {/* Prepaid balance: sources with a key (dsh) only have this, no rate limit
                   window. */}
              {(s.balances ?? []).map((b) => (
                <Row key={b.label} label={b.label} value={fmtMoney(b.amount, b.currency)} />
              ))}
              {s.bars.length > 0 ? (
                <div className={styles.bars}>
                  {s.bars.map((b) => (
                    <Bar key={b.label} bar={b} />
                  ))}
                </div>
              ) : (
                (s.balances ?? []).length === 0 && (
                  <div className={styles.hint}>{t("这个来源没有限流数据。")}</div>
                )
              )}
              {/* Codex's 24-hour utilization curve (corresponds to the history graph in
                   desktop's codex account section). */}
              {s.source === "codex" && (
                <>
                  <div className={styles.divider} />
                  <div className={styles.sectionLabel}>{t("占用率变化 · 近 24 小时")}</div>
                  <CodexUsageChart key={reloadKey} client={client} />
                </>
              )}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
