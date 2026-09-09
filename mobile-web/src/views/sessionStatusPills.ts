// 会话详情页头部下面那条「活状态轨」的内容，抽成纯函数。
//
// 为什么需要它：老板给的三条意见（tab 挤成一行、信息量上不去、像网页不像
// app）其实是同一段 chrome 的三个症状。旧头部把 200px 花在「头 + 五行静态字段
// 面板 + 六个各 46px 宽的 tab」上，而这个会话此刻真正在发生的事——它注册了
// 一个 watch 在等什么、它 fan out 了三个子代理、它的计划走到 P3/5、有两张卡在
// 等人答——一个都没露。数据早就在 `SessionInfo` 上（relay 的 SNAPSHOT_FIELDS
// 白名单里全都带着），缺的只是地方。
//
// 这条轨的规则是**只显示此刻为真的东西**：没有 watch 就没有 watch pill，没有
// 子代理就没有子代理 pill，空会话整条轨不渲染（返回空数组，调用方据此不画）。
// 这跟旧面板「模型 — / 工作区 — 」那种固定行数的表格是相反的取舍：固定表格的
// 宽度预算被最坏情况占着，而一条只画真相的轨在安静的会话上收缩到零。
//
// 静态字段（模型、推理强度、工作区、会话 id）不在这里——它们不会变，属于
// 「会话详情半屏」上那一行 chip，见 sessionInfoRows.ts。这里只放会变的。
//
// 桌面端的对应物是 SessionDetail.tsx 头部那排 chip；桌面横向排得下一整行，
// 所以它不需要「只画为真的」这条规则。

import { t } from "../i18n";
import type { SessionInfo, SessionStatus } from "../types";

/** 头部下面这条轨里，点某个 pill 会推开哪一面。
 *
 *  `sheet` = 打开「会话详情」半屏（watch 和子代理没有自己的整页，它们的明细
 *  就在半屏上）。其余五个是旧 tab 条上那五页，现在改成从这里推开的整页。 */
export type PillTarget = "decisions" | "plans" | "token" | "workflow" | "handoff" | "sheet";

/** 三档色调。`alert` 是「这条挡着你了」（要你答的卡、耗尽的额度、断掉的远端），
 *  `live` 是「它此刻在动」，`neutral` 是背景读数。刻意只有三档：一条 pill 轨上
 *  超过三种颜色就不再是分级而是噪音。 */
export type PillTone = "alert" | "live" | "neutral";

export interface StatusPill {
  /** 稳定标识。单测按它断言（label 随语言变，数值随数据变），CSS 不依赖它。 */
  key: string;
  label: string;
  tone: PillTone;
  /** 画一颗跟着文字颜色的圆点——只给「它此刻在动」那一颗，用来接替旧头部
   *  右上角那个脉冲状态点。 */
  dot?: boolean;
  /** 点它推开哪一面；缺席 = 纯读数，不可点。 */
  target?: PillTarget;
}

/** 「它此刻在动」的状态集。与 SessionDetailView 的 WORKING 同一份名单
 *  （waitingInput / active 不算——那是停下来等人，不是在跑）。 */
const WORKING: SessionStatus[] = ["thinking", "executing", "streaming", "processing", "delegating"];

export interface PillInput {
  /** 归属这条会话的待决策卡张数。不在 `SessionInfo` 上——决策卡是按设备聚合的
   *  一个收件箱（App.tsx 的 `aggregateDecisions`），所以由调用方按 sessionId
   *  数好了传进来。 */
  pendingDecisions?: number;
}

/**
 * 这条轨要画的 pill，按固定顺序。
 *
 * 顺序不是按重要性排的，是按**它会不会挡着你**排的：先是挡路的（额度耗尽、
 * 远端断开、等你答的卡），然后是它此刻在动，然后是进度读数。前面几颗是你要
 * 立刻处理的，后面几颗是你扫一眼的——横向滑动时先滑出视野的应该是后者。
 */
export function buildStatusPills(s: SessionInfo, opts: PillInput = {}): StatusPill[] {
  const pills: StatusPill[] = [];

  // ── 挡路的 ──────────────────────────────────────────────────────────────
  // 额度耗尽没有 reset 时刻（等的是有人去充值，不是等时钟），所以它既不改
  // status 也不进 auto-resume——除了这颗 pill，手机上没有别的地方会说。
  if (s.outOfCredits) {
    pills.push({ key: "outOfCredits", label: t("额度耗尽"), tone: "alert" });
  }
  // 远端 workspace 的 rca-over-ssh 传输断了、Fleet 杀了 agent。status 会说
  // remoteDisconnected 但不说是哪台主机为什么断——那句原话在半屏上。
  if (s.remoteDisconnect) {
    pills.push({ key: "remoteDisconnect", label: t("远端断开"), tone: "alert", target: "sheet" });
  }
  const pending = opts.pendingDecisions ?? 0;
  if (pending > 0) {
    pills.push({
      key: "decisions",
      label: t("{0} 张待决策", pending),
      tone: "alert",
      target: "decisions",
    });
  }

  // ── 它此刻在动 ──────────────────────────────────────────────────────────
  if (WORKING.includes(s.status)) {
    pills.push({ key: "running", label: t("运行中"), tone: "live", dot: true });
  }
  // 轮次进行中排进去的追问，轮次结束时才由 `claude --resume` 送出。旧 UI 里
  // 这些消息发出去就消失了，人不知道它们还在队里。
  if (s.pendingMessages && s.pendingMessages.length > 0) {
    pills.push({
      key: "queued",
      label: t("{0} 条排队", s.pendingMessages.length),
      tone: "live",
    });
  }
  if (s.runningSubagentCount && s.runningSubagentCount > 0) {
    pills.push({
      key: "subagents",
      label: t("{0} 个子代理", s.runningSubagentCount),
      tone: "live",
      target: "sheet",
    });
  }
  if (s.watches && s.watches.length > 0) {
    // 一个 watch 时报它轮询了几次——那是「它还活着、还在等」唯一的可见证据；
    // 多个时报个数，逐个的轮询次数在半屏上。
    const label =
      s.watches.length === 1
        ? t("watch ×{0}", s.watches[0].pollCount)
        : t("{0} 个 watch", s.watches.length);
    pills.push({ key: "watch", label, tone: "live", target: "sheet" });
  }

  // ── 进度读数 ────────────────────────────────────────────────────────────
  if (s.taskPlan && s.taskPlan.total > 0) {
    pills.push({
      key: "plan",
      label: t("计划 {0}/{1}", s.taskPlan.done, s.taskPlan.total),
      tone: "neutral",
      target: "plans",
    });
  }
  if (s.handoff) {
    pills.push({
      key: "handoff",
      label: t("接力 {0}/{1}", s.handoff.hop, s.handoff.chainLen),
      tone: "neutral",
      target: "handoff",
    });
  }
  // contextPercent 是 0–1 的比值（对齐桌面 SessionDetail 的 `* 100` 用法）。
  if (s.contextPercent != null) {
    pills.push({
      key: "context",
      label: t("上下文 {0}%", Math.round(s.contextPercent * 100)),
      tone: "neutral",
      target: "token",
    });
  }
  // 半分钱以下显示成 $0.00 等于没说；与桌面端和 sessionInfoRows 同一道门槛。
  if (s.totalCostUsd != null && s.totalCostUsd >= 0.005) {
    pills.push({
      key: "cost",
      label: `$${s.totalCostUsd.toFixed(2)}`,
      tone: "neutral",
      target: "token",
    });
  }

  return pills;
}
