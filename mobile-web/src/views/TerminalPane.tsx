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
  applyCtrl,
  createOutputPump,
  encodeInput,
  readProcOutput,
  resizeProc,
  writeProcInput,
  type ProcRecord,
} from "../terminal";
import styles from "./TerminalPane.module.css";

const POLL_MS = 300;

/** 手指要拖过这么多像素才算一次滚动，而不是一次点击(点击是用来叫软键盘的)。 */
const TOUCH_SCROLL_SLOP = 8;

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
  /** 键位条上的 Ctrl 是否按下(粘滞修饰键)。软键盘上没有 Ctrl，所以它只能由
   *  外面那一排按钮提供，再在这里作用到下一个敲下去的字符上。 */
  ctrl?: boolean;
  /** Ctrl 已经作用到一个键上了 —— 由父级把粘滞状态弹回去。 */
  onCtrlConsumed?: () => void;
}


// 默认导出:整个 xterm(含它的 CSS)只在真的开了终端时才下载,同 OfficePreview
// 的做法 —— 大多数人一整天都不会打开这个页面,不该让他们为它付首屏体积。
export default function TerminalPane({
  client,
  proc,
  onRecord,
  registerInput,
  ctrl = false,
  onCtrlConsumed,
}: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  // 刚刚这一下是划动不是点击 —— touchend 置起，紧随其后的 pointerup 消费掉。
  const scrolledRef = useRef(false);
  const theme = useResolvedTheme();

  // 放进 ref：父级传内联闭包时不该把整个终端拆了重建(scrollback 会没)。
  const onRecordRef = useRef(onRecord);
  onRecordRef.current = onRecord;
  const registerInputRef = useRef(registerInput);
  registerInputRef.current = registerInput;
  // 同理：Ctrl 的开关状态每次都变，但终端只该建一次。
  const ctrlRef = useRef(ctrl);
  ctrlRef.current = ctrl;
  const onCtrlConsumedRef = useRef(onCtrlConsumed);
  onCtrlConsumedRef.current = onCtrlConsumed;

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

    const sendResize = () => {
      void resizeProc(client, proc.id, term.cols, term.rows).catch(() => {
        // 已退出的进程没有控制套接字，resize 此时本来就没有意义。
      });
    };
    sendResize();

    const send = (data: string) => {
      void writeProcInput(client, proc.id, encodeInput(data)).catch(() => {});
    };
    // 软键盘敲进来的字符要过一次粘滞 Ctrl；键位条自己发的是完整序列
    // (`\x1b[A` 之类)，不该再被折一次，所以它走 send 而不是这里。
    const onData = term.onData((data) => {
      if (!ctrlRef.current) return send(data);
      send(applyCtrl(data));
      onCtrlConsumedRef.current?.();
    });
    registerInputRef.current?.(send);

    // 增量读的节奏全在 createOutputPump 里(含「上一轮没回来就不发下一轮」的
    // 在途闸 —— 慢链路上少了它同一段回显会上屏两遍)。
    const pump = createOutputPump({
      read: (offset) => readProcOutput(client, proc.id, offset),
      write: (bytes) => term.write(bytes),
      onRecord: (record) => onRecordRef.current?.(record),
    });
    void pump.poll();
    const timer = setInterval(() => void pump.poll(), POLL_MS);

    // ── 手势滚动 ────────────────────────────────────────────────────────
    // xterm 6 把视口换成了 VS Code 的 ScrollableElement(自绘滚动条)，它只认
    // wheel 和滚动条拖拽，**完全没有 touch 处理**(node_modules/@xterm/xterm/src/
    // vs/base/browser/ui/scrollbar/ 里搜不到任何 touch/Gesture)。再加上滚动条是
    // Auto 可见、手机上没有 hover，结果就是手指怎么划都翻不动历史。所以自己把
    // 拖动折成 term.scrollLines。
    let touchY: number | null = null;
    let touchAcc = 0; // 不足一行的余量攒着，否则慢速拖动一行都滚不动
    let scrolling = false;
    const cellHeight = () =>
      Math.max(1, el.getBoundingClientRect().height / Math.max(1, term.rows));

    const onTouchStart = (e: TouchEvent) => {
      if (e.touches.length !== 1) return;
      touchY = e.touches[0].clientY;
      touchAcc = 0;
      scrolling = false;
    };
    const onTouchMove = (e: TouchEvent) => {
      if (touchY === null || e.touches.length !== 1) return;
      const y = e.touches[0].clientY;
      const dy = touchY - y; // 手指上移 = 看更新的内容 = 向下滚
      if (!scrolling && Math.abs(dy) < TOUCH_SCROLL_SLOP) return;
      scrolling = true;
      touchY = y;
      touchAcc += dy;
      const lines = Math.trunc(touchAcc / cellHeight());
      if (lines !== 0) {
        touchAcc -= lines * cellHeight();
        term.scrollLines(lines);
      }
      // 不让外层把这一划当成页面滚动/下拉刷新。
      e.preventDefault();
    };
    const endTouch = () => {
      touchY = null;
      // 一次划动结束不该顺手把软键盘叫起来 —— 那会顶掉半屏、还把视口拉回底部。
      scrolledRef.current = scrolling;
      scrolling = false;
    };
    // passive:false —— 上面要 preventDefault。
    el.addEventListener("touchstart", onTouchStart, { passive: true });
    el.addEventListener("touchmove", onTouchMove, { passive: false });
    el.addEventListener("touchend", endTouch, { passive: true });
    el.addEventListener("touchcancel", endTouch, { passive: true });

    // 软键盘弹起会改可视高度，所以尺寸变化不只来自旋转屏幕。
    const observer = new ResizeObserver(() => {
      fit.fit();
      sendResize();
    });
    observer.observe(el);

    return () => {
      pump.stop();
      clearInterval(timer);
      observer.disconnect();
      el.removeEventListener("touchstart", onTouchStart);
      el.removeEventListener("touchmove", onTouchMove);
      el.removeEventListener("touchend", endTouch);
      el.removeEventListener("touchcancel", endTouch);
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
      // 手指点在字符网格上不一定命中它。刚划过一下的那次 pointerup 不算点击。
      onPointerUp={() => {
        if (scrolledRef.current) {
          scrolledRef.current = false;
          return;
        }
        termRef.current?.focus();
      }}
    />
  );
}
