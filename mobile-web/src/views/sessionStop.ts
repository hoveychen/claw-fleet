// 「停」这个动作的全部判断与执行 —— 任务列表卡片和会话详情半屏共用一份。
//
// 原先它只长在 TasksView 的卡片上（`stopMode` + `handleStop` 两个局部定义），
// 于是在会话详情页上看着一条正在跑的会话，唯一能停它的办法是退回列表再把那张
// 卡找出来。抽到这里之后两处走同一套三态与同一句确认文案，不会各自漂移。
//
// pid / workspacePath 只在**它自己那台主机**上有意义：调用方必须传那台设备的
// transport，发到别的设备上轻则停不掉，重则按 pid 打到一个毫不相干的进程。

import { t } from "../i18n";
import type { FleetTransport } from "../transport";
import type { SessionInfo, SessionStatus } from "../types";
import { isFleetOwnedEntrypoint } from "../types";

const WORKING: SessionStatus[] = [
  "thinking",
  "executing",
  "streaming",
  "processing",
  "delegating",
];

export type StopMode = "interrupt" | "stop" | "spent";

/** Same escalation as the desktop StopControl: interrupt only for Fleet-owned
 *  sessions with a precise pid mid-turn; otherwise kill; no pid → dead. */
export function stopMode(s: SessionInfo): StopMode {
  if (s.pid == null) return "spent";
  if (isFleetOwnedEntrypoint(s.entrypoint) && s.pidPrecise && WORKING.includes(s.status)) {
    return "interrupt";
  }
  return "stop";
}

/** 子代理没有自己的进程，停不了也不该给按钮。 */
export function canControl(s: SessionInfo): boolean {
  return !s.isSubagent;
}

/** 执行一次停止/中断。
 *
 *  返回 `false` 表示用户在确认框上点了取消（或这条会话本来就没得停），调用方
 *  据此收掉忙碌态；抛错表示 relay 那边真的失败了，由调用方决定怎么提示。 */
export async function runStop(
  transport: FleetTransport,
  s: SessionInfo,
  confirm: (msg: string) => Promise<boolean>,
): Promise<boolean> {
  const mode = stopMode(s);
  if (mode === "spent") return false;
  if (mode === "interrupt") {
    await transport.request("interrupt", { pid: s.pid });
    return true;
  }
  if (s.pidPrecise) {
    if (!(await confirm(t("确定停止「{0}」的这个会话吗？", s.workspaceName)))) return false;
    await transport.request("stop", { pid: s.pid });
    return true;
  }
  if (
    !(await confirm(
      t("无法精确定位进程，将停止「{0}」目录下的所有会话，确定吗？", s.workspaceName),
    ))
  ) {
    return false;
  }
  await transport.request("stop_workspace", { workspacePath: s.workspacePath });
  return true;
}
