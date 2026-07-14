// 「账号与用量」子页面：把桌面端 AccountInfo + UsagePanel 两块搬到手机上——
// 今日累计花费（复用 App 已在轮询的 today_usage）、Claude 账号档案与 5h/7d 限流条、
// 以及其它 agent 源（codex）的归一化用量条。
// 数据走 relay 的 `account_usage`（见 ../account.ts）。

import { useCallback, useEffect, useState } from "react";
import { ChevronLeft } from "lucide-react";
import { fetchAccountUsage } from "../account";
import { t } from "../i18n";
import type { RelayClient } from "../relay";
import type { AccountUsage, TodayUsage, UsageBar } from "../types";
import { UsageChart } from "./UsageChart";
import styles from "./UsageView.module.css";

interface Props {
  client: RelayClient | null;
  /** App header 里那份今日累计，直接复用——避免为同一个数字再扫一遍会话。 */
  todayUsage: TodayUsage | null;
  onBack: () => void;
}

/** 各源在标题里的显示名；未知源回落到原始 id。 */
const SOURCE_LABEL: Record<string, string> = {
  codex: "Codex",
};

/** 紧凑 token 数：1.2M / 34.5K / 780。 */
function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return `${n}`;
}

/** 距离限流窗口重置还有多久。已过期（或时间串解析不了）就不显示。 */
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

/** 与桌面端 UsagePanel 同一套阈值：60% 起警告色，85% 起危险色。 */
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

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className={styles.row}>
      <span className={styles.rowLabel}>{label}</span>
      <span className={styles.rowValue}>{value}</span>
    </div>
  );
}

export function UsageView({ client, todayUsage, onBack }: Props) {
  const [data, setData] = useState<AccountUsage | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  // 点 ⟳ 时递增，作为曲线组件的 key —— 让它连同账号一起重新拉一遍。
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
      <div className={styles.header}>
        <button className={styles.backButton} onClick={onBack} aria-label={t("返回")}>
          <ChevronLeft size={22} />
          {t("更多")}
        </button>
        <div className={styles.headerText}>
          <div className={styles.headerTitle}>{t("账号与用量")}</div>
        </div>
        <button
          className={styles.refresh}
          onClick={() => void refresh()}
          disabled={loading}
          aria-label={t("刷新")}
        >
          ⟳
        </button>
      </div>

      <div className={styles.body}>
        {/* ── 今日累计 ── */}
        <div className={styles.section}>
          <div className={styles.sectionLabel}>{t("今日累计")}</div>
          <div className={styles.card}>
            {todayUsage ? (
              <>
                <div className={styles.today}>
                  <span className={styles.todayCost}>${todayUsage.costUsd.toFixed(2)}</span>
                  <span className={styles.todayTokens}>
                    {t("{0} 输出 token", fmtTokens(todayUsage.outputTokens))}
                  </span>
                </div>
                <div className={styles.divider} />
                <Row
                  label={t("会话花费")}
                  value={`$${todayUsage.agentCostUsd.toFixed(2)} · ${t("{0} 个会话", todayUsage.sessionCount)}`}
                />
                <Row label={t("Fleet 自身花费")} value={`$${todayUsage.fleetCostUsd.toFixed(2)}`} />
              </>
            ) : (
              <div className={styles.hint}>{t("桌面端离线，拿不到今日用量。")}</div>
            )}
          </div>
        </div>

        {error && <div className={styles.hint}>{t("用量加载失败：{0}", error)}</div>}
        {!error && !data && loading && <div className={styles.hint}>{t("加载中…")}</div>}

        {/* ── Claude 账号 ── */}
        {data && (
          <div className={styles.section}>
            <div className={styles.sectionLabel}>Claude Code</div>
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
                      claude.usageSource === "foxy-switcher"
                        ? t("foxy-switcher（本地守护进程）")
                        : t("Anthropic 接口")
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

        {/* ── 近 24h 占用率曲线 ── */}
        <div className={styles.section}>
          <div className={styles.sectionLabel}>{t("占用率变化 · 近 24 小时")}</div>
          <div className={styles.card}>
            <UsageChart key={reloadKey} client={client} />
          </div>
        </div>

        {/* ── 其它 agent 源 ── */}
        {data?.sources.map((s) => (
          <div key={s.source} className={styles.section}>
            <div className={styles.sectionLabel}>{SOURCE_LABEL[s.source] ?? s.source}</div>
            <div className={styles.card}>
              {s.plan && <Row label={t("套餐")} value={s.plan} />}
              {s.bars.length > 0 ? (
                <div className={styles.bars}>
                  {s.bars.map((b) => (
                    <Bar key={b.label} bar={b} />
                  ))}
                </div>
              ) : (
                <div className={styles.hint}>{t("这个来源没有限流数据。")}</div>
              )}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
