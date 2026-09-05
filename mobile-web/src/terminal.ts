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

/** 退出后再多读几轮：host 先把 `<id>.out` 写完才把记录翻成 exited，不多读就会
 *  丢掉最后几行(往往正是报错)。 */
export const EXIT_DRAIN_POLLS = 3;

export interface OutputPumpDeps {
  /** 读一段增量输出。offset 为 null 表示「从最近的一段开始跟」。 */
  read: (offset: number | null) => Promise<ProcOutputChunk>;
  /** 把解出来的字节喂给终端。 */
  write: (bytes: Uint8Array) => void;
  onRecord?: (record: ProcRecord) => void;
}

export interface OutputPump {
  /** 跑一轮增量读。定时器每个 tick 调一次。 */
  poll: () => Promise<void>;
  /** 面板卸载：之后的响应一律丢弃，不再推进 offset。 */
  stop: () => void;
}

/** 输出泵：按 offset 增量读 pty 输出，喂给终端。
 *
 *  从 TerminalPane 里抽出来单测，因为它有一个只在慢链路上才现形的坑：`poll` 是
 *  异步的，而 offset 只在 await 回来之后才推进。定时器不等上一轮结束就发下一轮，
 *  于是**响应慢于轮询间隔时两轮会带着同一个 offset 出去，同一段输出被写两遍** ——
 *  手机经 relay 打回桌面端正是这种链路，敲 `ls` 屏幕上会显示 `llss`(pty 收到的
 *  仍是 `ls`，所以回车照样执行)。桌面端走本地 IPC 够快，几乎撞不上。 */
export function createOutputPump({ read, write, onRecord }: OutputPumpDeps): OutputPump {
  let offset: number | null = null;
  let drainPolls = 0;
  let stopped = false;
  // 在途闸：一轮没回来就不发下一轮。定时器只管催，真正的节奏由链路快慢决定 ——
  // 慢链路上轮询自然退化成「一问一答」，而不是几轮拿着同一个 offset 抢着上屏。
  let inFlight = false;

  return {
    async poll() {
      if (stopped || inFlight || drainPolls >= EXIT_DRAIN_POLLS) return;
      inFlight = true;
      try {
        const chunk = await read(offset);
        if (stopped) return;
        offset = chunk.nextOffset;
        if (chunk.dataB64) write(decodeOutput(chunk.dataB64));
        onRecord?.(chunk.record);
        if (chunk.record.status === "exited") drainPolls += 1;
      } catch {
        // 进程记录在面板开着的时候被清掉了 —— 停止推进，别把错误刷成满屏。
        drainPolls = EXIT_DRAIN_POLLS;
      } finally {
        inFlight = false;
      }
    },
    stop() {
      stopped = true;
    },
  };
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

/** 把一个字符折成它的控制码：Ctrl-A..Z 是 0x01..0x1a，另有几个符号也有定义。
 *
 *  软键盘上没有 Ctrl，所以它由键位条上的粘滞按钮提供，作用到下一个敲下去的
 *  字符上 —— 这个函数就是那一步。不认识的字符原样返回：Ctrl 对它没有定义，
 *  吞掉只会让人以为按键丢了。 */
export function applyCtrl(data: string): string {
  if (data.length !== 1) return data;
  const c = data.toUpperCase();
  if (c >= "A" && c <= "Z") return String.fromCharCode(c.charCodeAt(0) - 64);
  const punct: Record<string, string> = {
    "@": "\x00",
    " ": "\x00",
    "[": "\x1b",
    "\\": "\x1c",
    "]": "\x1d",
    "^": "\x1e",
    _: "\x1f",
  };
  return punct[c] ?? data;
}
