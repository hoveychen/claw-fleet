// 会话详情页 header 展开后那块信息面板的内容，抽成纯函数。
//
// 手机上的 header 只有一行：标题被 ellipsis 掉、工作区压成一个 34% 宽的后缀、
// 模型/PID/路径/时间根本没有位置。这些字段本来就都在 `SessionInfo` 上（快照里
// 已经带着），缺的只是一个能把它们摊开的地方。构造逻辑放这里而不是组件里，是
// 为了能被单测钉住「哪些字段在缺席时不出现」——面板上一行「模型 —」比没有这行
// 更糟。
//
// 桌面端的对应物是 SessionDetail.tsx 的 meta_row（同一批字段，用 chip 排布）。

import { dateLocale, t } from "../i18n";
import { toolForAgentSource } from "../agentSource";
import type { SessionInfo, SessionStatus } from "../types";

export interface InfoRow {
  /** 稳定标识，单测按它断言（label 随语言变，value 随数据变）。 */
  key: string;
  label: string;
  value: string;
  /** 等宽 + 任意位置换行 —— 会话 id 与各种路径。 */
  mono?: boolean;
}

const STATUS_LABEL: Record<SessionStatus, string> = {
  thinking: "思考中",
  executing: "执行中",
  streaming: "输出中",
  delegating: "分派子代理",
  processing: "处理中",
  waitingInput: "等待输入",
  active: "活跃",
  idle: "空闲",
  rateLimited: "限流中",
  serverErrored: "服务端错误",
  stuck: "疑似卡住",
};

export function statusLabel(status: SessionStatus): string {
  return t(STATUS_LABEL[status] ?? status);
}

/** 源注册名 → 展示名。认不出的源原样显示，不静默兜底成 Claude —— 这一行是
 *  "这个会话是谁跑的"，猜错比不显示更坏。 */
export function sourceLabel(agentSource?: string | null): string {
  const raw = (agentSource ?? "").trim();
  if (!raw) return "Claude";
  const tool = toolForAgentSource(raw);
  if (tool === "claude" && raw !== "claude" && raw !== "claude-code") return raw;
  return tool === "claude" ? "Claude" : tool === "codex" ? "Codex" : tool;
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
 * 面板要显示的行。缺席的字段不占行（不是显示成空值），所以一个刚起、还没产生
 * 用量的会话看到的是四五行，而不是一屏「—」。
 */
export function buildInfoRows(s: SessionInfo, now = Date.now()): InfoRow[] {
  const rows: InfoRow[] = [];
  const push = (key: string, label: string, value: string | undefined | null, mono?: boolean) => {
    const v = (value ?? "").trim();
    if (v) rows.push({ key, label, value: v, mono });
  };

  push("status", t("状态"), statusLabel(s.status));
  push("source", t("来源"), sourceLabel(s.agentSource));
  if (s.isSubagent) push("agentType", t("子代理类型"), s.agentType ?? t("子代理"));
  push("model", t("模型"), s.model);
  push("workspace", t("工作区"), s.workspaceName);
  push("workspacePath", t("工作区路径"), s.workspacePath, true);
  push("sessionId", t("会话 ID"), s.id, true);
  push("jsonlPath", t("会话记录"), s.jsonlPath, true);
  push("slug", t("Slug"), s.slug);
  // contextPercent 是 0–1 的比值（对齐桌面端 SessionDetail 的 `* 100` 用法），
  // 不是百分数。
  if (s.contextPercent != null) {
    push("context", t("上下文占用"), `${Math.round(s.contextPercent * 100)}%`);
  }
  // 半分钱以下的花费显示成 $0.00，等于没说；与桌面端同一道门槛。
  if (s.totalCostUsd != null && s.totalCostUsd >= 0.005) {
    push("cost", t("花费"), `$${s.totalCostUsd.toFixed(2)}`);
  }
  push("created", t("创建于"), fmtAbsTime(s.createdAtMs));
  if (s.lastActivityMs) {
    push(
      "lastActivity",
      t("最后活动"),
      `${fmtRelTime(s.lastActivityMs, now)} · ${fmtAbsTime(s.lastActivityMs)}`,
    );
  }
  if (s.pid != null) {
    push("pid", "PID", s.pidPrecise === false ? t("{0}（不精确）", s.pid) : String(s.pid));
  }
  push("entrypoint", t("启动方式"), s.entrypoint);
  if (s.pendingMessages && s.pendingMessages.length > 0) {
    push("pending", t("排队消息"), t("{0} 条", s.pendingMessages.length));
  }
  if (s.runningSubagentCount) {
    push("subagents", t("运行中子代理"), String(s.runningSubagentCount));
  }
  // `hop` 是 1 起的（handoff.rs 里 `hop: i as u32 + 1`），直接显示，别再 +1。
  if (s.handoff && s.handoff.chainLen > 1) {
    push("handoff", t("接力"), `${s.handoff.hop}/${s.handoff.chainLen}`);
  }
  if (s.watches && s.watches.length > 0) {
    push("watches", t("守望"), String(s.watches.length));
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
