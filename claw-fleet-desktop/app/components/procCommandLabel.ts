// 一条 workspace proc 的命令怎么显示给桌面端的人看。
//
// 独立成模块是因为**两个页面都要它**:终端页的标签条,和仓库页命令面板的执行记录。
// 下面这两个函数依赖本包的 ProcRecord 与调用方传进来的 i18n 文案,所以留在本包;
// 它们共同依赖的那个「是不是默认 shell」判断跨包共享给移动端,见 shared-ts/。

import type { ProcRecord } from "../types";
import { isDefaultShellCommand } from "../../../shared-ts/procShell";

// 本包内的调用点(和它的测试)不必知道这个判断来自包外。
export { isDefaultShellCommand };

/** 终端页标签用的短名:默认 shell → `shellLabel`,其余取第一个词再截断。 */
export function procLabel(proc: ProcRecord, shellLabel: string): string {
  const cmd = proc.command.trim();
  if (isDefaultShellCommand(cmd)) return shellLabel;
  const head = cmd.split(/\s+/)[0] ?? cmd;
  return head.length > 16 ? `${head.slice(0, 15)}…` : head;
}

/** 命令面板执行记录里显示的命令全文:默认 shell 换成 `shellLabel`,其余原样。
 *
 *  这里**不**截断 —— 命令面板的价值就在于看得见跑的到底是哪条命令。 */
export function procCommandText(command: string, shellLabel: string): string {
  return isDefaultShellCommand(command) ? shellLabel : command;
}
