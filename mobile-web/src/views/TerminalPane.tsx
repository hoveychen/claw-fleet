// 一个 pty 进程的 xterm 视图。桌面端 ProcTerminal.tsx 的移动版：同样是「轮询增量
// 输出 + 按键回灌控制套接字」，差别在于这里的每一次读写都走 FleetTransport
// (同源 HTTP 或 relay)，以及主题色要跟着 app 的亮/暗切换重涂。
//
// 为什么用 xterm 而不是 <pre>：git / curl / pnpm 这类命令靠 `\r` 原地刷新进度条，
// 纯文本渲染会把一条进度摊成几百行；vim、htop、claude 本身更是完全没法看。

import { useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { useResolvedTheme } from "../theme";
import type { FleetTransport } from "../transport";
import {
  decodeOutput,
  encodeInput,
  readProcOutput,
  resizeProc,
  writeProcInput,
  type ProcRecord,
} from "../terminal";
import styles from "./TerminalPane.module.css";

/** 退出后再多读几轮：host 先把 `<id>.out` 写完才把记录翻成 exited，不多读就会
 *  丢掉最后几行(往往正是报错)。数值同桌面端。 */
const EXIT_DRAIN_POLLS = 3;
const POLL_MS = 300;

/** xterm 画布不吃 CSS 变量，颜色必须显式给。取值对齐 index.css 的两套主题。 */
const THEMES = {
  dark: {
    background: "#0f1011",
    foreground: "#f7f8f8",
    cursor: "#d97757",
    selectionBackground: "rgba(217, 119, 87, 0.35)",
  },
  light: {
    background: "#fbfaf7",
    foreground: "#1f2023",
    cursor: "#c25232",
    selectionBackground: "rgba(194, 82, 50, 0.28)",
  },
} as const;

interface Props {
  client: FleetTransport;
  proc: ProcRecord;
  /** 每次输出轮询都带回来的最新记录 —— 父级据此知道命令退了没、退出码多少，
   *  不必为此再起第二条轮询(chunk 里本来就有)。 */
  onRecord?: (record: ProcRecord) => void;
  /** 交出一个「往这个 pty 里塞按键」的函数，给触屏键位条用。组件卸载时以 null
   *  回调一次，免得键位条握着一个已经没了的终端。 */
  registerInput?: (send: ((data: string) => void) | null) => void;
}

export function TerminalPane({ client, proc, onRecord, registerInput }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const theme = useResolvedTheme();

  // 放进 ref：父级传内联闭包时不该把整个终端拆了重建(scrollback 会没)。
  const onRecordRef = useRef(onRecord);
  onRecordRef.current = onRecord;
  const registerInputRef = useRef(registerInput);
  registerInputRef.current = registerInput;

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;

    const term = new Terminal({
      fontSize: 12,
      fontFamily: '"SF Mono", "JetBrains Mono", Menlo, Consolas, monospace',
      theme: { ...THEMES.dark },
      cursorBlink: true,
      scrollback: 5000,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(el);
    fit.fit();
    termRef.current = term;

    let disposed = false;
    let offset: number | null = null; // null = 从最近的一段开始跟
    let drainPolls = 0;

    const sendResize = () => {
      void resizeProc(client, proc.id, term.cols, term.rows).catch(() => {
        // 已退出的进程没有控制套接字，resize 此时本来就没有意义。
      });
    };
    sendResize();

    const send = (data: string) => {
      void writeProcInput(client, proc.id, encodeInput(data)).catch(() => {});
    };
    const onData = term.onData(send);
    registerInputRef.current?.(send);

    const poll = async () => {
      if (disposed || drainPolls >= EXIT_DRAIN_POLLS) return;
      try {
        const chunk = await readProcOutput(client, proc.id, offset);
        if (disposed) return;
        offset = chunk.nextOffset;
        if (chunk.dataB64) term.write(decodeOutput(chunk.dataB64));
        onRecordRef.current?.(chunk.record);
        if (chunk.record.status === "exited") drainPolls += 1;
      } catch {
        // 进程记录在面板开着的时候被清掉了 —— 停止推进，别把错误刷成满屏。
        drainPolls = EXIT_DRAIN_POLLS;
      }
    };
    void poll();
    const timer = setInterval(() => void poll(), POLL_MS);

    // 软键盘弹起会改可视高度，所以尺寸变化不只来自旋转屏幕。
    const observer = new ResizeObserver(() => {
      fit.fit();
      sendResize();
    });
    observer.observe(el);

    return () => {
      disposed = true;
      clearInterval(timer);
      observer.disconnect();
      onData.dispose();
      registerInputRef.current?.(null);
      term.dispose();
      termRef.current = null;
    };
  }, [client, proc.id]);

  // 主题翻转只换颜色，不重建终端 —— 重建会把已有输出全丢掉。
  useEffect(() => {
    const term = termRef.current;
    if (term) term.options.theme = { ...THEMES[theme] };
  }, [theme]);

  return (
    <div
      ref={containerRef}
      className={styles.pane}
      // 点哪都聚焦，软键盘才会起来：xterm 的输入落在一个隐藏 textarea 上，
      // 手指点在字符网格上不一定命中它。
      onPointerUp={() => termRef.current?.focus()}
    />
  );
}
