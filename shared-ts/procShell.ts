// 桌面端与移动端共用的一份「这条命令是不是默认 shell」。
//
// 这个目录是仓库里唯一的跨前端包共享源码点。claw-fleet-desktop 和 mobile-web 是两个
// 互相独立的 pnpm 包(没有 workspace 协议,没有共享 npm 包),所以共享靠的是两边的
// tsconfig 都 include 这个目录、用相对路径 import。
//
// 只放**零依赖的纯函数**。任何需要 ProcRecord、i18n、React 的东西都不属于这里 ——
// 两个包的 ProcRecord 类型和 t() 机制都不一样,硬拉过来只会把两套类型系统绑死。
// 具体到 shell:两边的 procLabel 各自留在各自包里,共用的只有下面这个字符串判断。

/** 这条命令是不是 core 的「给我个终端」默认 shell?
 *
 *  两种形状都要认:core 现在生成 `exec "<绝对路径>" -i`(见
 *  proc_runner::default_shell_command),而那次改动之前起的、仍活着的 pty 记录里存的
 *  是旧的 `exec "$SHELL" -i`。Windows 那边是裸 `cmd`。
 *
 *  之所以值得跨包共享:这个判断此前在桌面和移动端各存一份,core 把默认命令从
 *  `$SHELL` 改成绝对路径之后,只同步了一份,另一份就把默认 shell 显示成了「exec」。 */
export function isDefaultShellCommand(command: string): boolean {
  const cmd = command.trim();
  return /^exec\s+"[^"]*"\s+-i$/.test(cmd) || /^exec\s+"?\$SHELL/.test(cmd) || cmd === "cmd";
}
