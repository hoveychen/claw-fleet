// "More" tab: consolidates settings items scattered in header gear / top banner—
// language/theme, desktop connection status, notification toggle, re-pair, about/version.

import { useState } from "react";
import {
  Bell,
  BellOff,
  Check,
  ChevronRight,
  FolderGit2,
  BookOpen,
  Gauge,
  ListTree,
  QrCode,
  SquareTerminal,
} from "lucide-react";
import { useDraft } from "../draft";
import { dateLocale, useI18n, type Lang } from "../i18n";
import type { RttSplit } from "../connQuality";
import type { SnapshotSource } from "../snapshotSources";
import type { PushState } from "../push";
import type { PairedDevice } from "../devices";
import type { PairedLink } from "../pairingLink";
import { canScanPairing, scanPairing } from "../nativeScan";
import { scanAvailability } from "../scanAvailability";
import { PairPasteForm } from "./PairPasteForm";
import { PairScanner } from "./PairScanner";
import { useTheme, type ThemeSetting } from "../theme";
import { useWakeLock } from "../wakeLock";
import { useConfirm } from "../confirmDialog";
import { BUILD_COMMIT } from "../buildCommit";
import styles from "./MoreView.module.css";

const LANG_CHOICES: Array<[Lang, string]> = [
  ["zh", "中文"],
  ["en", "English"],
];

interface Props {
  /** "Where am I connected to", answered by transport layer (relay host / same-origin
   *  origin). Don't ask a specific implementation here or displaying one line would pull
   *  the relay client into the same-origin build. */
  endpointLabel: string;
  /** Whether this deployment has push channels. Same-origin doesn't (VAPID subscription
   *  lives on relay), so hide the toggle entirely rather than show a non-functional
   *  one. */
  supportsPush: boolean;
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
  onOpenPlans: () => void;
  onOpenWiki: () => void;
  onOpenUsage: () => void;
  onOpenTerminal: () => void;
  /** Whether this desktop machine has terminal UI (backend FLEET_TERMINAL, see
   *  useHostFeatures). When off, don't show the row: entry exists but opening can't
   *  spawn shell is worse UX. */
  terminalEnabled: boolean;
  /** Every Fleet this phone has paired, in join order. */
  devices: PairedDevice[];
  /** Id of the current scope device; null when no pairing (same-origin always null). */
  activeDeviceId: string | null;
  /** Which route the current device takes. Only affects "where I'm connected" title. */
  activeKind: "relay" | "http";
  onSwitchDevice: (id: string) => void;
  onRenameDevice: (id: string, label: string) => void;
  onRemoveDevice: (device: PairedDevice) => void;
  /** Whether this device's notifications are muted. */
  deviceMuted: (deviceId: string) => boolean;
  /** Toggle just one device's notifications. Whole-phone toggle is in "Connection &
   *  Notification" section above. */
  onMuteDevice: (device: PairedDevice, muted: boolean) => void;
  /** Add another device. Shares App's adoptPaired with pairing gate—dedup, preserve
   *  renamed labels, move focus have one implementation. */
  onAddDevice: (paired: PairedLink) => void;
  /** Clear all pairings and reload. */
  onUnpairAll: () => void;
}

export function MoreView({
  endpointLabel,
  supportsPush,
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
  onOpenPlans,
  onOpenWiki,
  onOpenUsage,
  onOpenTerminal,
  terminalEnabled,
  devices,
  activeDeviceId,
  activeKind,
  onSwitchDevice,
  onRenameDevice,
  onRemoveDevice,
  deviceMuted,
  onMuteDevice,
  onAddDevice,
  onUnpairAll,
}: Props) {
  const { lang, setLang, t } = useI18n();
  const confirm = useConfirm();
  // Device being renamed (inline input, not window.prompt—HarmonyOS ArkWeb dialog
  // may not be available, no reason to depend on it).
  const [editingId, setEditingId] = useState<string | null>(null);
  const [labelDraft, setLabelDraft] = useState("");
  // Camera viewfinder for "scan device QR". HarmonyOS shell uses its own path
  // (scanPairing reloads WebView and injects #k=); other surfaces use this page one.
  const [scanning, setScanning] = useState(false);
  /** Whether shell has a built-in scan bridge (HarmonyOS)—if so, system manages camera,
   *  not constrained by page security context. */
  const shellScan = canScanPairing();
  /** Without shell bridge, can the page itself open the camera? */
  const scan = scanAvailability();
  const { setting, setTheme } = useTheme();
  const wakeLock = useWakeLock();
  // Task-list handoff grouping—same "tasks:groupHandoff" draft that task page reads
  // on remount. On by default.
  const [groupHandoff, setGroupHandoff] = useDraft<boolean>("tasks:groupHandoff", true);

  const themeChoices: Array<[ThemeSetting, string]> = [
    ["system", t("跟随系统")],
    ["light", t("亮色")],
    ["dark", t("暗色")],
  ];

  // Diagnose decision card source. Normally one (desktop); second source or ignored
  // snapshots signal another agent answering in the channel—that agent causes cards to disappear.
  const trustedSource = snapshotSources.find((s) => s.trusted);
  const foreignSources = snapshotSources.filter((s) => s !== trustedSource);
  const ignoredTotal = snapshotSources.reduce((n, s) => n + s.ignored, 0);
  // One desktop restart is just a pid change (identity key has no pid), so records merge—
  // report process count honestly so "my ps shows different pid" isn't mistaken for impostor.
  const agentLabel = (s: SnapshotSource) => {
    if (!s.agent) return t("未署名");
    const base = `${s.agent.host ?? "?"} · pid ${s.agent.pid ?? "?"}`;
    return s.pids.length > 1 ? `${base} · ${t("重启过")} ${s.pids.length - 1} ${t("次")}` : base;
  };
  const hhmm = (ts: number) =>
    new Date(ts).toLocaleTimeString(dateLocale(), { hour: "2-digit", minute: "2-digit" });

  const connState = !connected ? "offline" : agentOnline ? "online" : "agent-offline";
  const connLabel = !connected
    ? t("连接中…")
    : agentOnline
      ? t("桌面端在线")
      : t("桌面端离线");

  // Viewfinder full-screen cover (position: fixed) lives at top level, not inside
  // devices section—when visible, "More" page stays below; cancel returns to it.
  if (scanning) {
    return (
      <PairScanner
        onPaired={(paired) => {
          setScanning(false);
          onAddDevice(paired);
        }}
        onClose={() => setScanning(false)}
      />
    );
  }

  return (
    <div className={styles.view}>
      {/* ── Tools ── */}
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
          <button className={styles.navRow} onClick={onOpenPlans}>
            <span className={styles.navIcon}>
              <ListTree size={18} />
            </span>
            <span className={styles.navText}>
              <span className={styles.navLabel}>{t("计划")}</span>
              <span className={styles.navSub}>{t("整仓 TASKS.md 计划的进度矩阵")}</span>
            </span>
            <ChevronRight size={18} className={styles.navChevron} />
          </button>
          <div className={styles.divider} />
          <button className={styles.navRow} onClick={onOpenWiki}>
            <span className={styles.navIcon}>
              <BookOpen size={18} />
            </span>
            <span className={styles.navText}>
              <span className={styles.navLabel}>{t("知识库")}</span>
              <span className={styles.navSub}>{t("agent 沉淀下来的调研与文档")}</span>
            </span>
            <ChevronRight size={18} className={styles.navChevron} />
          </button>
          {terminalEnabled && (
            <>
              <div className={styles.divider} />
              <button className={styles.navRow} onClick={onOpenTerminal}>
                <span className={styles.navIcon}>
                  <SquareTerminal size={18} />
                </span>
                <span className={styles.navText}>
                  <span className={styles.navLabel}>{t("终端")}</span>
                  <span className={styles.navSub}>
                    {t("在桌面端主机的某个目录里开一个 shell")}
                  </span>
                </span>
                <ChevronRight size={18} className={styles.navChevron} />
              </button>
            </>
          )}
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

      {/* ── Settings ── */}
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

      {/* ── Connection & Notifications ── */}
      <div className={styles.section}>
        <div className={styles.sectionLabel}>{t("连接与通知")}</div>
        <div className={styles.card}>
          <div className={styles.row}>
            {/* "Where I'm connected" title tracks **current device kind**: relay-routed device
                shows "Relay" (proper noun, consistent across languages, not in i18n),
                direct-connected shows "Server"—two devices, same build; wrong label lies. */}
            <span className={styles.rowLabel}>
              {supportsPush && activeKind !== "http" ? "Relay" : t("服务端")}
            </span>
            <span className={styles.relayValue}>{endpointLabel}</span>
          </div>
          {/* Same-origin deployment choice only: desktop and mobile are two builds from
              same server. Detection by screen short edge (see desktop index.html mobile-redirect);
              large phones or folding phones mis-detect—provide exit so users aren't stuck.
              `?desktop` saved to localStorage by that script, choice needed once. */}
          {!supportsPush && (
            <>
              <div className={styles.divider} />
              <div className={styles.row}>
                <span className={styles.rowLabel}>{t("界面")}</span>
                <button
                  className={styles.actionButton}
                  onClick={() => {
                    window.location.href = "/?desktop=1";
                  }}
                >
                  {t("切到桌面版")}
                </button>
              </div>
            </>
          )}
          <div className={styles.divider} />
          <div className={styles.row}>
            <span className={styles.rowLabel}>{t("桌面端")}</span>
            <span className={styles.connWrap}>
              <span className={styles.connDot} data-state={connState} />
              <span className={styles.connLabel}>{connLabel}</span>
            </span>
          </div>
          {/* One request round-trip split three ways. Which segment is large says what to fix:
              phone large = phone network, desktop link large = desktop to relay (or relay queueing),
              handler large = desktop handler slow. Unmeasured segments don't show, never use 0. */}
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
          {/* This diagnostic is purely **relay** semantics: answers "is another agent answering
              in the same channel instead of desktop?"—relay broadcasts each request to all agents.
              Direct HTTP host has no channel, no broadcast, can only answer itself—copying this
              would give false "N other agents" alarms users can't trace. */}
          {activeKind === "relay" && snapshotSources.length > 0 && (
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
          {/* Same-origin deployment has no push channels (VAPID subscription lives on relay,
              which this deployment deliberately doesn't touch). Hide the entire section, not show
              "unsupported"—that reads like a browser bug; really this deployment just lacks it. */}
          {supportsPush && (
            <>
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
            </>
          )}
        </div>
      </div>

      {/* ── Devices ── */}
      {devices.length > 0 && (
        <div className={styles.section}>
          <div className={styles.sectionLabel}>{t("设备")}</div>
          <div className={styles.card}>
            {devices.map((d, i) => (
              <div key={d.id}>
                {i > 0 && <div className={styles.divider} />}
                {editingId === d.id ? (
                  <div className={styles.deviceRow}>
                    <input
                      className={styles.deviceInput}
                      value={labelDraft}
                      autoFocus
                      onChange={(e) => setLabelDraft(e.target.value)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") {
                          onRenameDevice(d.id, labelDraft);
                          setEditingId(null);
                        }
                      }}
                    />
                    <button
                      className={styles.deviceBtn}
                      onClick={() => {
                        onRenameDevice(d.id, labelDraft);
                        setEditingId(null);
                      }}
                    >
                      {t("保存")}
                    </button>
                    <button className={styles.deviceBtn} onClick={() => setEditingId(null)}>
                      {t("取消")}
                    </button>
                  </div>
                ) : (
                  <div className={styles.deviceRow}>
                    {/* Whole row tappable = switch to this device. Current device not tappable
                    to avoid pointless reconnection. */}
                    <button
                      className={styles.deviceMain}
                      disabled={d.id === activeDeviceId}
                      onClick={() => onSwitchDevice(d.id)}
                    >
                      <span className={styles.deviceCheck}>
                        {d.id === activeDeviceId && <Check size={16} />}
                      </span>
                      <span className={styles.deviceLabel}>{d.label}</span>
                    </button>
                    {/* Toggle just this device's notifications. Home machine has long tasks,
                        work machine sends cards at midnight—handle separately, not one all-off. */}
                    {/* Direct HTTP transport has no push channels; switch would be misleading. */}
                    {supportsPush && d.kind === "relay" && (
                      <button
                        className={styles.deviceBtn}
                        onClick={() => onMuteDevice(d, !deviceMuted(d.id))}
                        aria-label={deviceMuted(d.id) ? t("开启通知") : t("静音")}
                      >
                        {deviceMuted(d.id) ? <BellOff size={15} /> : <Bell size={15} />}
                      </button>
                    )}
                    <button
                      className={styles.deviceBtn}
                      onClick={() => {
                        setLabelDraft(d.label);
                        setEditingId(d.id);
                      }}
                    >
                      {t("改名")}
                    </button>
                    <button
                      className={styles.deviceBtnDanger}
                      onClick={async () => {
                        if (
                          await confirm(
                            t("移除「{0}」？它的通知会停掉，本机为它缓存的任务与草稿一并清除。", d.label),
                          )
                        ) {
                          onRemoveDevice(d);
                        }
                      }}
                    >
                      {t("移除")}
                    </button>
                  </div>
                )}
              </div>
            ))}
          </div>
          {/* "Add second device" is either open a #k=-bearing URL or scan in app. Former
              doesn't work on any form: native shell from rawfile has no address bar; iOS
              home screen web app also has no address bar, and storage partitions from
              Safari separately—link scanned back to Safari doesn't exist to it. So both
              lines must be on **every** form.

              QR scan prefers shell's own (HarmonyOS shell reloads WebView and injects
              #k=, system manages camera, not bound by https below); without that bridge
              use page viewfinder needing getUserMedia—browsers refuse non-https addresses
              entirely, so line won't render, explanation follows. Paste is fallback when
              camera is rejected/unavailable and sole entry for self-hosted relay
              (system can't hand QR-scanned URLs to app). */}
          {(shellScan || scan === "ok") && (
            <div className={styles.card} style={{ marginTop: 8 }}>
              <button
                className={styles.navRow}
                onClick={() => (shellScan ? scanPairing() : setScanning(true))}
              >
                <span className={styles.navIcon}>
                  <QrCode size={18} />
                </span>
                <span className={styles.navText}>
                  <span className={styles.navLabel}>{t("扫码添加设备")}</span>
                  <span className={styles.navSub}>
                    {t("扫另一台桌面端「移动端」面板里的二维码")}
                  </span>
                </span>
                <ChevronRight size={16} className={styles.navChevron} />
              </button>
            </div>
          )}
          <div className={styles.rowNote}>
            {shellScan || scan === "ok"
              ? t("每台桌面端各出一张码;扫过的会留在上面这个列表里。")
              : scan === "insecure-origin"
                ? t("这个地址不是 HTTPS，浏览器不允许网页调用摄像头，扫码这条路走不了。请用下面的粘贴。")
                : t("这台设备用不了摄像头，扫不了码。请用下面的粘贴。")}
          </div>
          <div className={styles.pasteRow}>
            <PairPasteForm onPaired={onAddDevice} />
          </div>
        </div>
      )}

      {/* ── Pairing ── */}
      <div className={styles.section}>
        <div className={styles.sectionLabel}>{t("配对")}</div>
        <div className={styles.card}>
          <button
            className={styles.dangerRow}
            onClick={async () => {
              if (
                await confirm(
                  t("清除本机全部配对密钥？需回到桌面端重新扫码才能再连接。"),
                )
              ) {
                onUnpairAll();
              }
            }}
          >
            {t("重新配对 / 清除全部密钥")}
          </button>
        </div>
      </div>

      {/* ── About ── */}
      <div className={styles.section}>
        <div className={styles.sectionLabel}>{t("关于")}</div>
        <div className={styles.card}>
          <div className={styles.row}>
            <span className={styles.rowLabel}>Fleet Mobile</span>
            <span className={styles.rowValue}>v{__APP_VERSION__}</span>
          </div>
          {/* Build commit: for bug reports, "which build" is more precise than "which version"—
              package.json version changes rarely, but this bundle differs each release. Desktop
              already uses it to check if phone bundle is stale (hello frame's appCommit); here
              we just display the same value. When source is "unknown", that's not a commit, so
              the line doesn't render. */}
          {BUILD_COMMIT && (
            <>
              <div className={styles.divider} />
              <div className={styles.row}>
                <span className={styles.rowLabel}>{t("构建")}</span>
                <span className={styles.buildValue}>{BUILD_COMMIT}</span>
              </div>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
