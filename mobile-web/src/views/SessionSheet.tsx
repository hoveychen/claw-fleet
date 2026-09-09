// 「会话详情」半屏 —— 会话详情页上唯一的元信息面。
//
// 它取代了三样东西：
//   1. header 下面那块 inline 展开的 `infoPanel`（固定五行静态字段，把正文往下
//      推 ~90px，且没数据时也占着位）；
//   2. `SessionHeaderMenu` 那张「会话操作」sheet（复制 id / 路径 / 恢复命令、
//      切换作用域）；
//   3. header 下面那条六个 tab 的条（决策/计划/Token/Workflow/接力 —— 每个在
//      390px 宽的屏上只剩约 46px）。
//
// 三者本来是三个入口指向三堆重叠的信息：tab 条上「计划」页要点两下才知道计划
// 走到哪，而「走到哪」这个数字（`taskPlan.done/total`）在快照里一直躺着；
// 「切换作用域」在 ☰ 里，而「有几个子代理在跑」在 header 上完全没有。合成一张
// 半屏之后，标题点一下 / ☰ 点一下都到这里，每一行既是读数也是入口。
//
// 为什么是半屏而不是继续 inline 展开：inline 面板的高度是从正文那里借的，所以
// 它必须小，所以它只放得下五行静态字段——这正是老板说的「就算展开了也放不进
// watch、subagent」。半屏借的是**临时**的屏幕，可以占 85vh，于是「此刻在发生
// 什么」终于有地方摊开。带抓手 + 圆角 + 底部安全区，是移动端原生对这类「拿一次
// 就走」的面的既定语汇。
//
// 桌面端没有对应物：桌面 SessionDetail 的 header 横向排得下一整行 chip，且它
// 的 tab 条有 1000+ px 可用。这张半屏是手机独有的收敛。

import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import {
  Check,
  ChevronRight,
  Copy,
  FileJson2,
  Folder,
  Terminal,
  Timer,
  X,
} from "lucide-react";
import { t } from "../i18n";
import { HistoryLayer } from "../useNavStack";
import type { SessionInfo } from "../types";
import { agentIdTail, agentLabel } from "./agentScope";
import { buildInfoChips, resumeCommand } from "./sessionInfoRows";
import type { DetailPane } from "./sessionStatusPills";
import styles from "./SessionSheet.module.css";

/** 一行「点进去看」的入口：左边名字，右边此刻的读数。
 *
 *  读数是这张半屏存在的理由。旧 tab 条上「计划 / Token / Workflow / 接力」四个
 *  标签一个数字都不带，所以你得逐个点进去才知道哪个有东西——四次跳转换一次
 *  「哦，Workflow 是空的」。带上读数之后，绝大多数时候扫一眼就够，不用点。 */
interface PaneRow {
  pane: DetailPane;
  label: string;
  /** 右侧读数。`null` = 这一面此刻是空的（仍可点进去，但不吆喝）。 */
  value: string | null;
  /** 读数用 accent 强调 —— 只给「它在等你」那种。 */
  hot?: boolean;
  progress?: { done: number; total: number };
}

/** 一条要复制的文本。与旧 ☰ 菜单同一批条目、同一套「await 后再显示结果」纪律：
 *  移动端浏览器在非安全上下文 / 无用户手势时会拒掉 `clipboard.writeText`，
 *  发后不管就会出现「显示已复制、剪贴板里什么都没有」。 */
interface CopyRow {
  id: string;
  label: string;
  sub: string;
  icon: ReactNode;
  text: string;
}

export function SessionSheet({
  session,
  family,
  pendingDecisions,
  onClose,
  onOpenPane,
  onOpenSession,
}: {
  session: SessionInfo;
  /** 主进程 + 各子代理（调用方按桌面端同一套规则组装、排序、封顶）。为空表示
   *  这是个没有子代理的独会话，那一节整段不出现。 */
  family: SessionInfo[];
  /** 归属这条会话的待决策卡张数（决策卡是跨设备聚合的收件箱，不在 SessionInfo
   *  上，所以由调用方数好传进来）。 */
  pendingDecisions: number;
  onClose: () => void;
  onOpenPane: (pane: DetailPane) => void;
  onOpenSession: (s: SessionInfo) => void;
}) {
  const [result, setResult] = useState<{ id: string; ok: boolean } | null>(null);
  const clearTimer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(clearTimer.current), []);

  const copy = useCallback(async (row: CopyRow) => {
    window.clearTimeout(clearTimer.current);
    try {
      await navigator.clipboard.writeText(row.text);
      setResult({ id: row.id, ok: true });
      // ✓ 停留一拍。半屏不像旧菜单那样复制完就关——它是个「看板」，人常常还要
      // 接着看别的行，关掉反而要重开。
      clearTimer.current = window.setTimeout(() => setResult(null), 1200);
    } catch {
      setResult({ id: row.id, ok: false });
    }
  }, []);

  const title = session.titleOverride || session.aiTitle || session.slug || t("会话");
  const resume = resumeCommand(session);

  // 静态字段压成一行 chip（模型 / 推理强度 / 工作区 / 上下文 / 花费）。缺席的
  // 不占位——规则和单测在 sessionInfoRows.ts。
  const chips = buildInfoChips(session);

  // ── 「此刻」──────────────────────────────────────────────────────────
  const nowRows: PaneRow[] = [];
  if (pendingDecisions > 0) {
    nowRows.push({
      pane: "decisions",
      label: t("待决策"),
      value: t("{0} 张", pendingDecisions),
      hot: true,
    });
  }
  const watches = session.watches ?? [];

  // ── 「进度」──────────────────────────────────────────────────────────
  const progressRows: PaneRow[] = [];
  progressRows.push({
    pane: "plans",
    label: t("计划"),
    value: session.taskPlan
      ? session.taskPlan.currentTask
        ? `${session.taskPlan.currentTask} · ${session.taskPlan.done}/${session.taskPlan.total}`
        : `${session.taskPlan.done}/${session.taskPlan.total}`
      : null,
    progress: session.taskPlan ?? undefined,
  });
  progressRows.push({ pane: "token", label: t("Token 与花费"), value: null });
  progressRows.push({ pane: "workflow", label: t("Workflow"), value: null });
  progressRows.push({
    pane: "handoff",
    label: t("接力链"),
    value: session.handoff
      ? t("第 {0} 棒 / 共 {1}", session.handoff.hop, session.handoff.chainLen)
      : null,
  });
  // 决策历史即使此刻没有待答的卡也要能进去 —— 这一面装的是**答过的**卡，
  // 「上次我到底点了哪个」是它最常被用到的问法。
  if (pendingDecisions === 0) {
    progressRows.push({ pane: "decisions", label: t("决策记录"), value: null });
  }

  // ── 「会话」──────────────────────────────────────────────────────────
  const copyRows: CopyRow[] = [
    {
      id: "id",
      label: t("会话 ID"),
      sub: session.id,
      icon: <Copy size={15} />,
      text: session.id,
    },
    {
      id: "workspace",
      label: t("工作区路径"),
      sub: session.workspacePath,
      icon: <Folder size={15} />,
      text: session.workspacePath,
    },
    {
      id: "transcript",
      label: t("会话记录路径"),
      sub: session.jsonlPath,
      icon: <FileJson2 size={15} />,
      text: session.jsonlPath,
    },
  ];
  if (resume) {
    copyRows.push({
      id: "resume",
      label: t("恢复命令"),
      sub: resume,
      icon: <Terminal size={15} />,
      text: resume,
    });
  }

  const paneRow = (r: PaneRow) => (
    <button
      key={`${r.pane}-${r.label}`}
      type="button"
      className={styles.row}
      onClick={() => {
        onClose();
        onOpenPane(r.pane);
      }}
    >
      <span className={styles.rowLabel}>{r.label}</span>
      {r.progress && r.progress.total > 0 && (
        <span className={styles.bar} aria-hidden="true">
          <i style={{ width: `${Math.round((r.progress.done / r.progress.total) * 100)}%` }} />
        </span>
      )}
      {r.value !== null ? (
        <span className={styles.rowValue} data-hot={r.hot || undefined}>
          {r.value}
        </span>
      ) : (
        <span className={styles.rowValue} data-empty="">
          {t("无")}
        </span>
      )}
      <ChevronRight size={15} className={styles.chev} />
    </button>
  );

  return (
    <>
      {/* 半屏也算一层：不登记的话返回键弹掉的是整个会话详情页。 */}
      <HistoryLayer onBack={onClose} />
      {createPortal(
        <div className={styles.backdrop} onClick={onClose}>
          {/* 55：必须压过决策抽屉那条常驻底栏（DecisionDrawer 的 45）——它就贴在
              半屏要展开的位置上。承自 SessionHeaderMenu 的实测结论。 */}
          <div
            className={styles.sheet}
            role="dialog"
            aria-label={t("会话详情")}
            onClick={(e) => e.stopPropagation()}
          >
            <div className={styles.grabber} aria-hidden="true" />
            <button
              type="button"
              className={styles.close}
              onClick={onClose}
              aria-label={t("关闭")}
            >
              <X size={17} />
            </button>

            <div className={styles.title}>{title}</div>
            {chips.length > 0 && (
              <div className={styles.chips}>
                {chips.map((c) => (
                  <span key={c} className={styles.chip}>
                    {c}
                  </span>
                ))}
              </div>
            )}

            {/* 挡路的原话。pill 轨上只写得下「远端断开」「额度耗尽」四个字，
                为什么断、哪台主机、原始报错长什么样，只有这里放得下。 */}
            {session.outOfCredits && (
              <div className={styles.alert}>
                <span className={styles.alertLabel}>{t("额度耗尽")}</span>
                <span className={styles.alertBody}>{session.outOfCredits}</span>
              </div>
            )}
            {session.remoteDisconnect && (
              <div className={styles.alert}>
                <span className={styles.alertLabel}>
                  {t("远端断开")}
                  {session.remoteDisconnect.hostLabel
                    ? ` · ${session.remoteDisconnect.hostLabel}`
                    : ""}
                </span>
                <span className={styles.alertBody}>{session.remoteDisconnect.detail}</span>
              </div>
            )}
            {session.mirrorWrite && (
              <div className={styles.alert}>
                <span className={styles.alertLabel}>
                  {t("{0} 个文件留在本机", session.mirrorWrite.total)}
                </span>
                <span className={styles.alertBody}>
                  {session.mirrorWrite.files.slice(0, 4).join("\n")}
                </span>
              </div>
            )}

            {(nowRows.length > 0 || watches.length > 0 || family.length > 0) && (
              <div className={styles.section}>{t("此刻")}</div>
            )}
            {nowRows.map(paneRow)}

            {/* watch 没有自己的整页 —— 它的全部内容就是「在等什么、轮询了几次、
                什么时候放弃」这三句，够放在这里，不值得一次跳转。 */}
            {watches.map((w) => (
              <div key={w.id} className={styles.watchRow}>
                <Timer size={15} className={styles.watchIcon} />
                <span className={styles.watchText}>
                  <span className={styles.watchNote}>{w.note || t("未说明在等什么")}</span>
                  <span className={styles.watchMeta}>
                    {t("轮询 {0} 次 · 每 {1}s", w.pollCount, w.pollSecs)}
                  </span>
                </span>
              </div>
            ))}

            {/* 作用域切换。旧 ☰ 里的那份清单原样搬来 —— 它本来就属于「此刻这个
                会话家族里谁在动」，跟复制路径挨在一起是旧菜单的历史包袱。 */}
            {family.map((s) => {
              const isCurrent = s.id === session.id;
              return (
                <button
                  key={s.id}
                  type="button"
                  className={styles.row}
                  data-current={isCurrent ? "" : undefined}
                  onClick={() => {
                    onClose();
                    if (!isCurrent) onOpenSession(s);
                  }}
                >
                  <span className={styles.dot} data-status={s.status} />
                  <span className={styles.rowLabel}>
                    {agentLabel(s)}
                    {isCurrent && ` · ${t("当前")}`}
                  </span>
                  {s.isSubagent && (
                    <span className={styles.rowValue}>{agentIdTail(s.id)}</span>
                  )}
                  {!isCurrent && <ChevronRight size={15} className={styles.chev} />}
                </button>
              );
            })}

            <div className={styles.section}>{t("进度")}</div>
            {progressRows.map(paneRow)}

            <div className={styles.section}>{t("会话")}</div>
            {copyRows.map((row) => {
              const r = result?.id === row.id ? result : null;
              return (
                <button
                  key={row.id}
                  type="button"
                  className={styles.copyRow}
                  data-failed={r && !r.ok ? "" : undefined}
                  onClick={() => void copy(row)}
                >
                  <span className={styles.copyIcon}>
                    {r ? r.ok ? <Check size={15} /> : <X size={15} /> : row.icon}
                  </span>
                  <span className={styles.copyText}>
                    <span className={styles.copyLabel}>
                      {r && !r.ok ? t("复制失败（需要 HTTPS 或用户手势）") : row.label}
                    </span>
                    <span className={styles.copySub}>{row.sub}</span>
                  </span>
                </button>
              );
            })}
          </div>
        </div>,
        document.body,
      )}
    </>
  );
}
