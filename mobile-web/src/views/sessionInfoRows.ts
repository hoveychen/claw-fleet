// 会话详情页 header 展开后那块信息面板的内容，抽成纯函数。
//
// 手机上的 header 只有一行：标题被 ellipsis 掉、工作区压成一个 34% 宽的后缀、
// 模型/PID/路径/时间根本没有位置。这些字段本来就都在 `SessionInfo` 上（快照里
// 已经带着），缺的只是一个能把它们摊开的地方。构造逻辑放这里而不是组件里，是
// 为了能被单测钉住「哪些字段在缺席时不出现」——面板上一行「模型 —」比没有这行
// 更糟。
//
// 刻意只放**身份**字段：模型、两条路径、会话 id、两个时刻。状态有 header 上的
// 状态点，用量/花费有 Token 页签，接力棒次有 接力 页签 —— 那些在别处已经有更
// 好的呈现，搬一份到这里只会把「我要找那条路径」变成一屏要扫的东西。
//
// 桌面端的对应物是 SessionDetail.tsx 的 meta_row（那里字段更多，因为桌面横向
// 排得下一整行 chip）。

import { dateLocale, t } from "../i18n";
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

/** "2026-08-30 12:57" —— 绝对时刻，跟随 UI 语言。 */
export function fmtAbsTime(ms: number): string {
  const d = new Date(ms);
  if (!ms || Number.isNaN(d.getTime())) return "";
  return d.toLocaleString(dateLocale(), {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** 相对时刻（"3 分钟前"）。与 TasksView 的 timeAgo 同口径。 */
export function fmtRelTime(ms: number, now = Date.now()): string {
  const diff = now - ms;
  if (diff < 60_000) return t("刚刚");
  if (diff < 3_600_000) return t("{0} 分钟前", Math.floor(diff / 60_000));
  if (diff < 86_400_000) return t("{0} 小时前", Math.floor(diff / 3_600_000));
  return t("{0} 天前", Math.floor(diff / 86_400_000));
}

/**
 * 面板要显示的行。缺席的字段不占行（不是显示成空值）—— 一个还没记到模型的
 * 会话少一行，而不是多一行「模型 —」。
 */
export function buildInfoRows(s: SessionInfo, now = Date.now()): InfoRow[] {
  const rows: InfoRow[] = [];
  const push = (key: string, label: string, value: string | undefined | null, mono?: boolean) => {
    const v = (value ?? "").trim();
    if (v) rows.push({ key, label, value: v, mono });
  };

  push("model", t("模型"), s.model);
  push("workspacePath", t("工作区路径"), s.workspacePath, true);
  push("sessionId", t("会话 ID"), s.id, true);
  push("jsonlPath", t("会话记录"), s.jsonlPath, true);
  push("created", t("创建于"), fmtAbsTime(s.createdAtMs));
  if (s.lastActivityMs) {
    push(
      "lastActivity",
      t("最后活动"),
      `${fmtRelTime(s.lastActivityMs, now)} · ${fmtAbsTime(s.lastActivityMs)}`,
    );
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
