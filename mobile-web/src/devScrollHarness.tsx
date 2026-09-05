// 只在开发期用的取证台：把**真正的** TerminalPane 挂起来，后面接一个假 pty
// 主机(吐 200 行可辨认的文本)，这样触摸滚动可以在桌面 Chrome 的移动仿真下用
// patchwright 发真实 touch 序列来判定 —— 而不是靠猜。
//
// 它不进任何产物：scroll-harness.html 只在 dev server 上存在，vite build 的
// 入口是 index.html。

import { createRoot } from "react-dom/client";
import TerminalPane from "./views/TerminalPane";
import type { FleetTransport } from "./transport";
import type { ProcOutputChunk, ProcRecord } from "./types";

const LINES = 200;
const LOG = Array.from({ length: LINES }, (_, i) => `line-${String(i).padStart(3, "0")}`).join(
  "\r\n",
);

const record: ProcRecord = {
  id: "harness",
  workspacePath: "/harness",
  command: "",
  status: "running",
  startedMs: Date.now(),
  cols: 80,
  rows: 24,
};

function b64(s: string): string {
  const bytes = new TextEncoder().encode(s);
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin);
}

/** 假主机。`?latency=800` 可以把响应拖慢，用来看轮询重入。 */
const latency = Number(new URLSearchParams(location.search).get("latency") ?? "0");

const client = {
  connect() {},
  close() {},
  sayGoodbye() {},
  isAuthed: true,
  endpointLabel: "harness",
  async request<T>(method: string, params?: Record<string, unknown>): Promise<T> {
    if (latency) await new Promise((r) => setTimeout(r, latency));
    if (method === "proc_output") {
      const from = (params?.offset as number | undefined) ?? 0;
      const slice = LOG.slice(from);
      const chunk: ProcOutputChunk = {
        dataB64: b64(slice),
        nextOffset: from + slice.length,
        record,
      };
      return chunk as unknown as T;
    }
    return undefined as unknown as T;
  },
  answer: () => true,
} as unknown as FleetTransport;

// 判定滚动位置不需要探针：直接读 .xterm-rows 里第一行的文字是 line-000 还是
// line-1xx —— 那正是用户眼睛看到的事实。

createRoot(document.getElementById("root")!).render(
  <TerminalPane
    client={client}
    proc={record}
    registerInput={() => {}}
    onRecord={() => {}}
  />,
);
