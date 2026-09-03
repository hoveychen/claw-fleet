// 终端面板的客户端：把 proc_runner 的 pty 主机搬到手机/浏览器上。
//
// 后端(claw-fleet-core/src/proc_runner.rs)本来就是一个完整的交互式终端 —— 分离的
// pty host、stdin 转发、resize、killpg 整组杀、按 offset 增量读输出。桌面端经
// Backend trait 用它，这里经 serve_request 的 proc_* 方法用同一套，所以两端看到的
// 是同一批进程。
//
// 关键性质：**pty host 是分离的**。关掉这个页面、退出浏览器、甚至换一台设备，
// 命令都还在桌面端跑着 —— 所以重开面板要先 `listProcs` 把它们接回来，而不是每次
// 都开一个新 shell。

import type { FleetTransport } from "./transport";
import type { ProcOutputChunk, ProcRecord } from "./types";

export type { ProcRecord, ProcOutputChunk, ProcStatus } from "./types";

/** 这台主机上所有的终端进程，最新的在前。传 workspacePath 只看某个工作区的。 */
export function listProcs(
  client: FleetTransport,
  workspacePath?: string,
): Promise<ProcRecord[]> {
  return client.request<ProcRecord[]>("procs", workspacePath ? { workspacePath } : {});
}

/** 在 workspacePath 下开一个新的 pty，跑 command。 */
export function runProc(
  client: FleetTransport,
  workspacePath: string,
  command: string,
  cols: number,
  rows: number,
): Promise<ProcRecord> {
  return client.request<ProcRecord>("proc_run", { workspacePath, command, cols, rows });
}

/** 从 offset 起增量读输出；不传 offset 表示「从最近的一段开始跟」。
 *
 *  回来的 chunk 自带 record，所以「命令退了没 / 退出码多少」不需要第二条轮询 ——
 *  桌面端的 ProcTerminal 也是这么用的。 */
export function readProcOutput(
  client: FleetTransport,
  id: string,
  offset: number | null,
): Promise<ProcOutputChunk> {
  return client.request<ProcOutputChunk>("proc_output", offset === null ? { id } : { id, offset });
}

/** 把按键原样送进 pty(base64 的原始字节，不是文本行)。 */
export function writeProcInput(
  client: FleetTransport,
  id: string,
  dataB64: string,
): Promise<void> {
  return client.request<void>("proc_input", { id, dataB64 });
}

/** 告诉 pty 新的窗口尺寸，vim/htop 这类全屏程序靠它重排。 */
export function resizeProc(
  client: FleetTransport,
  id: string,
  cols: number,
  rows: number,
): Promise<void> {
  return client.request<void>("proc_resize", { id, cols, rows });
}

/** 杀掉命令的整个进程组。force 跳过 SIGTERM 宽限直接 SIGKILL。 */
export function killProc(client: FleetTransport, id: string, force = false): Promise<void> {
  return client.request<void>("proc_kill", { id, force });
}

/** 删掉一个已退出进程的记录与日志，免得重连列表越积越长。 */
export function clearProc(client: FleetTransport, id: string): Promise<{ cleared: number }> {
  return client.request<{ cleared: number }>("proc_clear", { id });
}

/** 把 xterm 的 onData 字符串编成后端要的 base64 原始字节。
 *
 *  不能直接 `btoa(data)`：btoa 只接受 latin1，任何非 ASCII 输入(中文、emoji，
 *  以及 IME 上屏的整段文字)都会抛 InvalidCharacterError，按键就静默丢了。 */
export function encodeInput(data: string): string {
  const bytes = new TextEncoder().encode(data);
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin);
}

/** base64 的 pty 输出还原成字节喂给 xterm。 */
export function decodeOutput(dataB64: string): Uint8Array {
  return Uint8Array.from(atob(dataB64), (c) => c.charCodeAt(0));
}
