// 会话详情页 header 展开后那块信息面板的内容，抽成纯函数。
//
// 手机上的 header 只有一行：标题被 ellipsis 掉、工作区压成一个 34% 宽的后缀、
// 模型/PID/路径/时间根本没有位置。这些字段本来就都在 `SessionInfo` 上（快照里
// 已经带着），缺的只是一个能把它们摊开的地方。构造逻辑放这里而不是组件里，是
// 为了能被单测钉住「哪些字段在缺席时不出现」——面板上一行「模型 —」比没有这行
// 更糟。
//
// 刻意只放这几行：模型、工作区、会话 id、上下文占用、花费。两条路径不在这里
// ——它们在 ☰ 菜单的复制条目副行上原样摆着，那才是路径真正被用到的地方（复制
// 走）；时间戳也不在这里，每条消息旁边就有，会话列表上还有「几分钟前」。状态
// 有 header 上那个点，接力棒次有 接力 页签。
//
// 桌面端的对应物是 SessionDetail.tsx 的 meta_row（那里字段更多，因为桌面横向
// 排得下一整行 chip）。

import { t } from "../i18n";
import { toolForAgentSource } from "../agentSource";
import type { SessionInfo } from "../types";

export interface InfoRow {
  /** 稳定标识，单测按它断言（label 随语言变，value 随数据变）。 */
  key: string;
  label: string;
  value: string;
  /** 等宽 + 任意位置换行 —— 会话 id 与各种路径。 */
  mono?: boolean;
}

/**
 * 面板要显示的行。缺席的字段不占行（不是显示成空值）—— 一个还没记到模型的
 * 会话少一行，而不是多一行「模型 —」。
 */
export function buildInfoRows(s: SessionInfo): InfoRow[] {
  const rows: InfoRow[] = [];
  const push = (key: string, label: string, value: string | undefined | null, mono?: boolean) => {
    const v = (value ?? "").trim();
    if (v) rows.push({ key, label, value: v, mono });
  };

  push("model", t("模型"), s.model);
  // 紧跟模型：这两个合起来才说明「这个会话在用什么算」。桌面 header 上它们
  // 也是相邻的两颗 chip。
  push("effort", t("推理强度"), s.effort);
  push("workspace", t("工作区"), s.workspaceName);
  push("sessionId", t("会话 ID"), s.id, true);
  // contextPercent 是 0–1 的比值（对齐桌面端 SessionDetail 的 `* 100` 用法），
  // 不是百分数。
  if (s.contextPercent != null) {
    push("context", t("上下文占用"), `${Math.round(s.contextPercent * 100)}%`);
  }
  // 半分钱以下的花费显示成 $0.00，等于没说；与桌面端同一道门槛。
  if (s.totalCostUsd != null && s.totalCostUsd >= 0.005) {
    push("cost", t("花费"), `$${s.totalCostUsd.toFixed(2)}`);
  }
  return rows;
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
