// 一条 workspace proc 的命令怎么显示给人看。
//
// 独立成模块是因为**两个页面都要它**:终端页的标签条,和仓库页命令面板的执行记录。
// 两处各写一份正则正是它上一次出错的方式 —— core 从 `exec "$SHELL" -i` 改成
// `exec "/bin/zsh" -i` 之后,只同步了一处的另一处就把默认 shell 显示成了「exec」。

import type { ProcRecord } from "../types";

/** 这条命令是不是 core 的「给我个终端」默认 shell?
 *
 *  两种形状都要认:core 现在生成 `exec "<绝对路径>" -i`(见
 *  proc_runner::default_shell_command),而那次改动之前起的、仍活着的 pty 记录里存的
 *  是旧的 `exec "$SHELL" -i`。Windows 那边是裸 `cmd`。 */
export function isDefaultShellCommand(command: string): boolean {
  const cmd = command.trim();
  return /^exec\s+"[^"]*"\s+-i$/.test(cmd) || /^exec\s+"?\$SHELL/.test(cmd) || cmd === "cmd";
}

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
