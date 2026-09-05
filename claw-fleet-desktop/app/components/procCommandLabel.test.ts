import { describe, expect, it } from "vitest";
import { isDefaultShellCommand, procCommandText, procLabel } from "./procCommandLabel";
import type { ProcRecord } from "../types";

/**
 * 这几个断言存在的理由是一次真实的回归：core 的 default_shell_command 从
 * `exec "$SHELL" -i` 改成 `exec "/bin/zsh" -i` 之后，认命令的正则没跟着改，默认
 * shell 的标签就显示成了「exec」。当时两个页面各存了一份正则，只同步了一份。
 *
 * 所以这里钉死的是**两种形状都要认**——新形状是现在生成的，旧形状存在于那次改动
 * 之前起的、仍活着的 pty 记录里。
 *
 * isDefaultShellCommand 现在住在 shared-ts/procShell.ts,移动端 import 的是同一个
 * 文件(经本模块 re-export 进来)。所以这一组绿就等于两端都绿 —— 共享层没有自己的
 * vitest 工程,把用例放在这里是为了不为一个纯函数再配一套测试运行器。
 */

function rec(command: string): ProcRecord {
  return {
    id: "p1",
    workspacePath: "/repo",
    command,
    status: "running",
    startedMs: 0,
    cols: 80,
    rows: 24,
  } as ProcRecord;
}

describe("isDefaultShellCommand", () => {
  it("认得 core 现在生成的绝对路径形状", () => {
    expect(isDefaultShellCommand('exec "/bin/zsh" -i')).toBe(true);
    expect(isDefaultShellCommand('exec "/usr/local/bin/fish" -i')).toBe(true);
    expect(isDefaultShellCommand('  exec "/bin/sh" -i  ')).toBe(true);
  });

  it("认得改动之前留下的旧形状", () => {
    expect(isDefaultShellCommand('exec "$SHELL" -i')).toBe(true);
    expect(isDefaultShellCommand("exec $SHELL -i")).toBe(true);
  });

  it("认得 Windows 的裸 cmd", () => {
    expect(isDefaultShellCommand("cmd")).toBe(true);
  });

  it("不把普通命令误判成 shell", () => {
    expect(isDefaultShellCommand("pnpm build")).toBe(false);
    expect(isDefaultShellCommand("cargo test")).toBe(false);
    // 形似但不是：exec 一个脚本、没有 -i
    expect(isDefaultShellCommand('exec "/bin/zsh" script.sh')).toBe(false);
    expect(isDefaultShellCommand("cmd /c dir")).toBe(false);
  });
});

describe("procLabel", () => {
  it("默认 shell 用给定的短名", () => {
    expect(procLabel(rec('exec "/bin/zsh" -i'), "shell")).toBe("shell");
  });

  it("其余取第一个词", () => {
    expect(procLabel(rec("pnpm build --watch"), "shell")).toBe("pnpm");
  });

  it("过长的第一个词截断", () => {
    const label = procLabel(rec("./scripts/run-a-very-long-thing.sh"), "shell");
    expect(label).toHaveLength(16);
    expect(label.endsWith("…")).toBe(true);
  });
});

describe("procCommandText", () => {
  it("默认 shell 换成短名", () => {
    expect(procCommandText('exec "/bin/zsh" -i', "shell")).toBe("shell");
  });

  it("普通命令保留全文——命令面板的价值就在于看得见跑的是哪条命令", () => {
    expect(procCommandText("pnpm build --watch", "shell")).toBe("pnpm build --watch");
  });
});
