// 「更多」tab：把原来散在 header 齿轮 / 顶部横幅里的设置项收纳到一处——
// 语言 / 主题、桌面端连接状态、通知开关、重新配对、关于/版本。

import { ChevronRight, FolderGit2, Gauge } from "lucide-react";
import { useDraft } from "../draft";
import { dateLocale, useI18n, type Lang } from "../i18n";
import type { RttSplit } from "../connQuality";
import type { SnapshotSource } from "../snapshotSources";
import type { PushState } from "../push";
import { relayDisplayHost } from "../relay";
import { clearSecret } from "../secretStore";
import { clearCachedSessions } from "../sessionCache";
import { useTheme, type ThemeSetting } from "../theme";
import { useWakeLock } from "../wakeLock";
import styles from "./MoreView.module.css";

const LANG_CHOICES: Array<[Lang, string]> = [
  ["zh", "中文"],
  ["en", "English"],
];

interface Props {
  connected: boolean;
  agentOnline: boolean;
  /** Most recent sessions frame kind + running counts, to show whether the
   *  desktop's delta path is actually engaged. */
  sessionsFrame: { last: "full" | "delta" | null; full: number; delta: number };
  /** Latest round trip split three ways, so a laggy phone can be told apart
   *  from a laggy desktop link and from a slow desktop handler. */
  rttSplit: RttSplit | null;
  /** Every agent that has served a `pending_snapshot` this session. Normally
   *  one (the desktop); a second entry is a stray agent answering in its place,
   *  which is what blanks the card list — so it gets surfaced here by name. */
  snapshotSources: SnapshotSource[];
  push: PushState;
  /** True when the user turned notifications off while permission stays granted. */
  pushOptedOut: boolean;
  onEnablePush: () => void;
  onDisablePush: () => void;
  onOpenRepo: () => void;
  onOpenUsage: () => void;
}

export function MoreView({
  connected,
  agentOnline,
  sessionsFrame,
  rttSplit,
  snapshotSources,
  push,
  pushOptedOut,
  onEnablePush,
  onDisablePush,
  onOpenRepo,
  onOpenUsage,
}: Props) {
  const { lang, setLang, t } = useI18n();
  const { setting, setTheme } = useTheme();
  const wakeLock = useWakeLock();
  // Task-list handoff grouping — same "tasks:groupHandoff" draft the task page
  // reads on remount. Default on.
  const [groupHandoff, setGroupHandoff] = useDraft<boolean>("tasks:groupHandoff", true);

  const themeChoices: Array<[ThemeSetting, string]> = [
    ["system", t("跟随系统")],
    ["light", t("亮色")],
    ["dark", t("暗色")],
  ];

  // 决策卡来源诊断。正常只有一条（桌面端）；出现第二条、或有空快照被忽略过，
  // 说明频道里有别的 agent 在替桌面端作答 —— 卡片自己消失就是它干的。
  const trustedSource = snapshotSources.find((s) => s.trusted);
  const foreignSources = snapshotSources.filter((s) => s !== trustedSource);
  const ignoredTotal = snapshotSources.reduce((n, s) => n + s.ignored, 0);
  const agentLabel = (s: SnapshotSource) =>
    s.agent ? `${s.agent.host ?? "?"} · pid ${s.agent.pid ?? "?"}` : t("未署名");
  const hhmm = (ts: number) =>
    new Date(ts).toLocaleTimeString(dateLocale(), { hour: "2-digit", minute: "2-digit" });

  const connState = !connected ? "offline" : agentOnline ? "online" : "agent-offline";
  const connLabel = !connected
    ? t("连接中…")
    : agentOnline
      ? t("桌面端在线")
      : t("桌面端离线");

  return (
    <div className={styles.view}>
      {/* ── 工具 ── */}
      <div className={styles.section}>
        <div className={styles.sectionLabel}>{t("工具")}</div>
        <div className={styles.card}>
          <button className={styles.navRow} onClick={onOpenRepo}>
            <span className={styles.navIcon}>
              <FolderGit2 size={18} />
            </span>
            <span className={styles.navText}>
              <span className={styles.navLabel}>{t("仓库")}</span>
              <span className={styles.navSub}>{t("查看未合并 worktree 与未推提交")}</span>
            </span>
            <ChevronRight size={18} className={styles.navChevron} />
          </button>
          <div className={styles.divider} />
          <button className={styles.navRow} onClick={onOpenUsage}>
            <span className={styles.navIcon}>
              <Gauge size={18} />
            </span>
            <span className={styles.navText}>
              <span className={styles.navLabel}>{t("账号与用量")}</span>
              <span className={styles.navSub}>{t("今日花费、账号档案与限流占用")}</span>
            </span>
            <ChevronRight size={18} className={styles.navChevron} />
          </button>
        </div>
      </div>

      {/* ── 设置 ── */}
      <div className={styles.section}>
        <div className={styles.sectionLabel}>{t("设置")}</div>
        <div className={styles.card}>
          <div className={styles.row}>
            <span className={styles.rowLabel}>{t("语言")}</span>
            <div className={styles.segment}>
              {LANG_CHOICES.map(([value, label]) => (
                <button
                  key={value}
                  className={styles.segmentButton}
                  data-active={lang === value}
                  onClick={() => setLang(value)}
                >
                  {label}
                </button>
              ))}
            </div>
          </div>
          <div className={styles.divider} />
          <div className={styles.row}>
            <span className={styles.rowLabel}>{t("主题")}</span>
            <div className={styles.segment}>
              {themeChoices.map(([value, label]) => (
                <button
                  key={value}
                  className={styles.segmentButton}
                  data-active={setting === value}
                  onClick={() => setTheme(value)}
                >
                  {label}
                </button>
              ))}
            </div>
          </div>
          <div className={styles.divider} />
          <div className={styles.row}>
            <span className={styles.rowLabel}>{t("接力会话分组")}</span>
            <div className={styles.segment}>
              <button
                className={styles.segmentButton}
                data-active={!groupHandoff}
                onClick={() => setGroupHandoff(false)}
              >
                {t("关")}
              </button>
              <button
                className={styles.segmentButton}
                data-active={groupHandoff}
                onClick={() => setGroupHandoff(true)}
              >
                {t("开")}
              </button>
            </div>
          </div>
          {wakeLock.supported && (
            <>
              <div className={styles.divider} />
              <div className={styles.row}>
                <span className={styles.rowLabel}>{t("屏幕常亮")}</span>
                <div className={styles.segment}>
                  <button
                    className={styles.segmentButton}
                    data-active={!wakeLock.enabled}
                    onClick={() => wakeLock.setEnabled(false)}
                  >
                    {t("关")}
                  </button>
                  <button
                    className={styles.segmentButton}
                    data-active={wakeLock.enabled}
                    onClick={() => wakeLock.setEnabled(true)}
                  >
                    {t("开")}
                  </button>
                </div>
              </div>
            </>
          )}
        </div>
      </div>

      {/* ── 连接与通知 ── */}
      <div className={styles.section}>
        <div className={styles.sectionLabel}>{t("连接与通知")}</div>
        <div className={styles.card}>
          <div className={styles.row}>
            {/* 与「关于」里的 Fleet Mobile 同理，Relay 是专名，中英一致，不进字典。 */}
            <span className={styles.rowLabel}>Relay</span>
            <span className={styles.relayValue}>{relayDisplayHost()}</span>
          </div>
          <div className={styles.divider} />
          <div className={styles.row}>
            <span className={styles.rowLabel}>{t("桌面端")}</span>
            <span className={styles.connWrap}>
              <span className={styles.connDot} data-state={connState} />
              <span className={styles.connLabel}>{connLabel}</span>
            </span>
          </div>
          {/* 一个请求的往返被拆成三段。哪一段大，要修的东西完全不同：手机段大
              是这台手机的网络，桌面链路段大是桌面到 relay（或 relay 排队），处理段
              大是桌面 handler 自己慢。未测到的段不显示，不拿 0 冒充。 */}
          <div className={styles.divider} />
          <div className={styles.row}>
            <span className={styles.rowLabel}>{t("链路耗时")}</span>
            <span className={styles.connWrap}>
              <span className={styles.connLabel}>
                {rttSplit ? `${rttSplit.totalMs}ms` : t("等待样本…")}
              </span>
              {rttSplit && (
                <span className={styles.frameCount}>
                  {[
                    rttSplit.phoneMs !== null && `${t("手机")} ${rttSplit.phoneMs}`,
                    rttSplit.desktopLinkMs !== null &&
                      `${t("桌面链路")} ${rttSplit.desktopLinkMs}`,
                    rttSplit.handleMs !== null && `${t("处理")} ${rttSplit.handleMs}`,
                  ]
                    .filter(Boolean)
                    .join(" · ") || t("分段不可用")}
                </span>
              )}
            </span>
          </div>
          <div className={styles.divider} />
          <div className={styles.row}>
            <span className={styles.rowLabel}>{t("会话更新")}</span>
            <span className={styles.connWrap}>
              <span className={styles.connLabel}>
                {sessionsFrame.last === null
                  ? t("等待推送…")
                  : sessionsFrame.last === "delta"
                    ? `${t("增量")} ✓`
                    : t("全量")}
              </span>
              {sessionsFrame.full + sessionsFrame.delta > 0 && (
                <span className={styles.frameCount}>
                  {t("增量")} {sessionsFrame.delta} · {t("全量")} {sessionsFrame.full}
                </span>
              )}
            </span>
          </div>
          {snapshotSources.length > 0 && (
            <>
              <div className={styles.divider} />
              <div className={styles.row}>
                <span className={styles.rowLabel}>{t("决策卡来源")}</span>
                <span className={styles.connWrap}>
                  <span className={styles.connLabel}>
                    {trustedSource ? agentLabel(trustedSource) : t("待确认")}
                  </span>
                  <span className={styles.frameCount}>
                    {foreignSources.length > 0
                      ? `${t("另有")} ${foreignSources.length} ${t("个 agent")}`
                      : t("独占")}
                  </span>
                </span>
              </div>
              {foreignSources.length > 0 && (
                <div className={styles.sourceList}>
                  {foreignSources.map((s, i) => (
                    <div className={styles.sourceItem} key={s.key ?? `anon-${i}`}>
                      <span className={styles.sourceHead}>
                        {agentLabel(s)} · {hhmm(s.firstAt)}–{hhmm(s.lastAt)}
                      </span>
                      <span className={styles.sourceHome}>{s.agent?.home ?? "—"}</span>
                      <span className={styles.sourceStat}>
                        {t("回了")} {s.snapshots} {t("份")}
                        {s.ignored > 0 ? ` · ${t("空快照被拦")} ${s.ignored}` : ""}
                      </span>
                    </div>
                  ))}
                  <div className={styles.sourceNote}>
                    {t(
                      "同一频道里有别的 agent 在替桌面端作答（relay 会把请求广播给所有 agent）。把上面的 host / pid / 目录发给桌面端排查。",
                    )}
                  </div>
                </div>
              )}
              {foreignSources.length === 0 && ignoredTotal > 0 && (
                <div className={styles.sourceList}>
                  <div className={styles.sourceNote}>
                    {t("已拦下")} {ignoredTotal} {t("份可疑的空快照（卡片没被清掉）。")}
                  </div>
                </div>
              )}
            </>
          )}
          <div className={styles.divider} />
          <div className={styles.row}>
            <span className={styles.rowLabel}>{t("通知")}</span>
            {push === "granted" ? (
              pushOptedOut ? (
                <button className={styles.actionButton} onClick={onEnablePush}>
                  {t("开启")}
                </button>
              ) : (
                <button className={styles.actionButton} onClick={onDisablePush}>
                  {t("停用")}
                </button>
              )
            ) : push === "denied" ? (
              <span className={styles.rowValue}>{t("已拒绝")}</span>
            ) : push === "unsupported" || push === "unsupported-harmony" ? (
              <span className={styles.rowValue}>{t("不支持")}</span>
            ) : push === "ios-needs-a2hs" ? (
              <span className={styles.rowValue}>{t("需添加到主屏幕")}</span>
            ) : (
              <button className={styles.actionButton} onClick={onEnablePush}>
                {t("开启")}
              </button>
            )}
          </div>
          {push === "denied" && (
            <div className={styles.rowNote}>
              {t("通知权限已被拒绝，请在系统设置中为本站点重新开启。")}
            </div>
          )}
          {push === "ios-needs-a2hs" && (
            <div className={styles.rowNote}>
              {t("要接收通知，请先用 Safari 分享菜单「添加到主屏幕」，再从主屏幕打开。")}
            </div>
          )}
          {push === "unsupported-harmony" && (
            <div className={styles.rowNote}>
              {t("当前浏览器（鸿蒙 ArkWeb）不支持网页通知，请用桌面端 Fleet 接收决策卡提醒。")}
            </div>
          )}
          {push === "unsupported" && (
            <div className={styles.rowNote}>
              {t("当前浏览器不支持网页通知，请用桌面端 Fleet 接收决策卡提醒。")}
            </div>
          )}
        </div>
      </div>

      {/* ── 配对 ── */}
      <div className={styles.section}>
        <div className={styles.sectionLabel}>{t("配对")}</div>
        <div className={styles.card}>
          <button
            className={styles.dangerRow}
            onClick={() => {
              if (window.confirm(t("清除本机配对密钥？需回到桌面端重新扫码才能再连接。"))) {
                clearSecret();
                clearCachedSessions();
                location.reload();
              }
            }}
          >
            {t("重新配对 / 清除密钥")}
          </button>
        </div>
      </div>

      {/* ── 关于 ── */}
      <div className={styles.section}>
        <div className={styles.sectionLabel}>{t("关于")}</div>
        <div className={styles.card}>
          <div className={styles.row}>
            <span className={styles.rowLabel}>Fleet Mobile</span>
            <span className={styles.rowValue}>v{__APP_VERSION__}</span>
          </div>
        </div>
      </div>
    </div>
  );
}
