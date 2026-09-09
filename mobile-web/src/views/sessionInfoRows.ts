// 会话详情半屏顶部那行 chip 的内容，抽成纯函数。
//
// 它的前身是 header 下面那块 inline 展开的面板：五行 label/value 表格，吃掉约
// 90px 去说五个短词。表格对齐在那里没有换来什么——这些字段都是「一眼确认」型
// 的读数（模型对不对、在哪个工作区、烧了多少），不需要纵向对齐成列。改成一行
// 可折行的 chip 之后，同样五项占约 26px，省下来的高度归给半屏上真正需要摊开的
// 东西：watch 在等什么、几个子代理在跑、计划走到哪。
//
// 构造逻辑放这里而不是组件里，是为了能被单测钉住「哪些字段在缺席时不出现」
// ——一颗写着「模型 —」的 chip 比没有这颗更糟。
//
// 刻意只放这几项：模型、推理强度、工作区、上下文占用、花费。两条路径不在这里
// ——它们在半屏「会话」那一节的复制行副行上原样摆着，那才是路径真正被用到的
// 地方（复制走）；时间戳也不在，每条消息旁边就有，会话列表上还有「几分钟前」。
// 状态、watch、子代理、计划、接力都是会变的，它们在状态轨和半屏的「此刻」/
// 「进度」两节里，见 sessionStatusPills.ts。
//
// 桌面端的对应物是 SessionDetail.tsx 的 meta_row（那里字段更多，因为桌面横向
// 排得下一整行 chip）。

import { t } from "../i18n";
import { toolForAgentSource } from "../agentSource";
import type { SessionInfo } from "../types";

/**
 * 半屏顶部那行 chip 的文案，按固定顺序。缺席的字段不产出 chip（不是产出一颗
 * 空的）—— 一个还没记到模型的会话少一颗，而不是多一颗「模型 —」。
 *
 * 前三项是裸值：模型名、推理强度档位、工作区名本身就自带含义，加个标签只是
 * 重复。后两项带标签：孤零零一个「40%」说不清是上下文还是别的什么。
 */
export function buildInfoChips(s: SessionInfo): string[] {
  const chips: string[] = [];
  const push = (v: string | undefined | null) => {
    const trimmed = (v ?? "").trim();
    if (trimmed) chips.push(trimmed);
  };

  push(s.model);
  // 紧跟模型：这两个合起来才说明「这个会话在用什么算」。桌面 header 上它们
  // 也是相邻的两颗 chip。
  push(s.effort);
  push(s.workspaceName);
  // contextPercent 是 0–1 的比值（对齐桌面 SessionDetail 的 `* 100` 用法），
  // 不是百分数。
  if (s.contextPercent != null) {
    chips.push(t("上下文 {0}%", Math.round(s.contextPercent * 100)));
  }
  // 半分钱以下的花费显示成 $0.00，等于没说；与桌面端同一道门槛。
  if (s.totalCostUsd != null && s.totalCostUsd >= 0.005) {
    chips.push(`$${s.totalCostUsd.toFixed(2)}`);
  }
  return chips;
}

/**
 * 能贴进终端直接恢复这个会话的命令。
 *
 * 只对 Claude 源给：`claude --resume <id>`（形状见 claw-fleet-core 的
 * session_launch.rs 顶部注释）。codex 的恢复是 `codex exec resume <id>`，那是
 * headless 形态，交互形态另说；与其给一条可能贴上去就报错的命令，不如这一项
 * 干脆不出现（返回 null，菜单据此不渲染该条）。dsh 没有 CLI 恢复入口。
 */
export function resumeCommand(s: SessionInfo): string | null {
  return toolForAgentSource(s.agentSource) === "claude" ? `claude --resume ${s.id}` : null;
}
