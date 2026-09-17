// An xterm view of a pty process. Mobile version of desktop's ProcTerminal.tsx:
// same pattern of "incremental polling + key input to control socket", but all
// I/O here goes through FleetTransport (same-origin HTTP or relay), and theme
// colors must update when the app switches between light and dark modes.
//
// Why xterm instead of <pre>: commands like git/curl/pnpm use `\r` to refresh
// progress bars in place; plain text rendering spreads one progress line across
// hundreds of lines. vim, htop, and claude itself become unreadable.

import { useCallback, useEffect, useRef, useState } from "react";
import { ChevronDown, ChevronUp, ChevronsDown } from "lucide-react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { t } from "../i18n";
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

/** Minimum pixels a finger must drag before it counts as scrolling, not a click
 * (clicks summon the soft keyboard). */
const TOUCH_SCROLL_SLOP = 8;

/** xterm canvas doesn't use CSS variables; colors must be explicit.
 * Values align with the two themes in index.css. */
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
  /** Latest record returned with each output poll. Parent uses this to know if
   *  the command has exited and its exit code, without needing a separate poll
   *  (it's already in the chunk). */
  onRecord?: (record: ProcRecord) => void;
  /** Provides a function to send keypresses to this pty for the on-screen
   *  keyboard. Calls back with null once on unmount so the keyboard doesn't
   *  hold a reference to a dead terminal. */
  registerInput?: (send: ((data: string) => void) | null) => void;
  /** Whether Ctrl on the key bar is pressed (sticky modifier). There is no Ctrl
   *  on the soft keyboard, so it's provided by the button row and applied here
   *  to the next character typed. */
  ctrl?: boolean;
  /** Ctrl has been applied to a key. Parent should reset the sticky state. */
  onCtrlConsumed?: () => void;
}


// Default export: the entire xterm library (including CSS) is only downloaded
// when the terminal is actually opened, like OfficePreview. Most users won't
// open this page all day, so they shouldn't pay the initial bundle size cost.
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
  // This last action was a scroll, not a click. Set by touchend, consumed by
  // the following pointerup.
  const scrolledRef = useRef(false);
  // Show "back to bottom" button only when not at bottom; keep that space empty
  // normally so it doesn't obstruct output.
  const [atBottom, setAtBottom] = useState(true);
  const theme = useResolvedTheme();

  // Store in ref: when parent passes inline closures, don't rebuild the entire
  // terminal (scrollback would be lost).
  const onRecordRef = useRef(onRecord);
  onRecordRef.current = onRecord;
  const registerInputRef = useRef(registerInput);
  registerInputRef.current = registerInput;
  // Same reasoning: Ctrl toggle state changes every render, but terminal should
  // be built only once.
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
        // Already-exited processes have no control socket; resize is no-op anyway.
      });
    };
    sendResize();

    const send = (data: string) => {
      void writeProcInput(client, proc.id, encodeInput(data)).catch(() => {});
    };
    // Characters from the soft keyboard go through the sticky Ctrl filter;
    // the key bar sends complete sequences (`\x1b[A` etc.) that shouldn't be
    // folded again, so it calls send() directly instead of going through onData.
    const onData = term.onData((data) => {
      if (!ctrlRef.current) return send(data);
      send(applyCtrl(data));
      onCtrlConsumedRef.current?.();
    });
    registerInputRef.current?.(send);

    // Incremental read timing is all in createOutputPump (including the "don't
    // send next poll until previous reply arrives" gate — without it on slow
    // links, the same echo could appear on screen twice).
    const pump = createOutputPump({
      read: (offset) => readProcOutput(client, proc.id, offset),
      write: (bytes) => term.write(bytes),
      onRecord: (record) => onRecordRef.current?.(record),
    });
    void pump.poll();
    const timer = setInterval(() => void pump.poll(), POLL_MS);

    // Show/hide the "back to bottom" button. onScroll fires only when scroll
    // position changes, which is sufficient.
    const onScroll = term.onScroll(() => {
      const buf = term.buffer.active;
      setAtBottom(buf.viewportY >= buf.baseY);
    });

    // ── Touch scrolling ────────────────────────────────────────────────────
    // xterm 6 switched the viewport to VS Code's ScrollableElement (custom
    // scrollbar), which only handles wheel and scrollbar dragging, with **no
    // touch support at all** (search node_modules/@xterm/xterm/src/vs/base/
    // browser/ui/scrollbar/ finds zero touch/Gesture code). Plus the scrollbar
    // is Auto-visibility with no hover on mobile, so finger drags won't scroll
    // the history. We implement it ourselves by converting drags to
    // term.scrollLines().
    let touchY: number | null = null;
    let touchAcc = 0; // Accumulate sub-line amounts; without this, slow drags
                      // wouldn't scroll even one line
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
      const dy = touchY - y; // Finger upward = view newer content = scroll down
      if (!scrolling && Math.abs(dy) < TOUCH_SCROLL_SLOP) return;
      scrolling = true;
      touchY = y;
      touchAcc += dy;
      const lines = Math.trunc(touchAcc / cellHeight());
      if (lines !== 0) {
        touchAcc -= lines * cellHeight();
        term.scrollLines(lines);
      }
      // Prevent the outer layer from treating this drag as page scroll/pull-to-refresh.
      e.preventDefault();
    };
    const endTouch = () => {
      touchY = null;
      // After a drag ends, don't accidentally summon the soft keyboard — it would
      // take up half the screen and pull the viewport back to the bottom.
      scrolledRef.current = scrolling;
      scrolling = false;
    };
    // passive:false — we call preventDefault above.
    el.addEventListener("touchstart", onTouchStart, { passive: true });
    el.addEventListener("touchmove", onTouchMove, { passive: false });
    el.addEventListener("touchend", endTouch, { passive: true });
    el.addEventListener("touchcancel", endTouch, { passive: true });

    // Soft keyboard popping up changes the visible height, so size changes aren't
    // just from screen rotation.
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
      onScroll.dispose();
      registerInputRef.current?.(null);
      term.dispose();
      termRef.current = null;
    };
  }, [client, proc.id]);

  /** Scroll by half screen. Full-screen jumps can lose the seam between
   *  contexts; half-screen keeps half the content visible for continuity. */
  const pageScroll = useCallback((dir: -1 | 1) => {
    const term = termRef.current;
    if (!term) return;
    term.scrollLines(dir * Math.max(1, Math.floor(term.rows / 2)));
  }, []);

  // Theme switch only changes colors, doesn't rebuild the terminal — that would
  // lose all existing output.
  useEffect(() => {
    const term = termRef.current;
    if (term) term.options.theme = { ...THEMES[theme] };
  }, [theme]);

  return (
    <div className={styles.wrap}>
      <div
        ref={containerRef}
        className={styles.pane}
        // Click anywhere to focus so the soft keyboard appears. xterm's input is
        // on a hidden textarea; tapping the character grid doesn't always hit it.
        // Skip this on pointerup after a recent scroll.
        onPointerUp={() => {
          if (scrolledRef.current) {
            scrolledRef.current = false;
            return;
          }
          termRef.current?.focus();
        }}
      />
      {/* Explicit pagination controls. Touch scrolling works in desktop Chrome's
          mobile emulation, but different WebViews (HarmonyOS ArkWeb, various
          embedded WebViews) handle touch inconsistently. Not scrolling history
          on small screens breaks this page, so we keep a reliable gesture-free
          path. Half-screen jumps are faster than line-by-line dragging. */}
      <div className={styles.scrollPad}>
        <button
          className={styles.scrollKey}
          aria-label={t("向上翻页")}
          // onPointerDown + preventDefault: keep focus on the hidden textarea,
          // or the soft keyboard closes (same as the key bar does).
          onPointerDown={(e) => {
            e.preventDefault();
            pageScroll(-1);
          }}
        >
          <ChevronUp size={16} />
        </button>
        <button
          className={styles.scrollKey}
          aria-label={t("向下翻页")}
          onPointerDown={(e) => {
            e.preventDefault();
            pageScroll(1);
          }}
        >
          <ChevronDown size={16} />
        </button>
        {!atBottom && (
          <button
            className={styles.scrollKey}
            data-accent
            aria-label={t("回到底部")}
            onPointerDown={(e) => {
              e.preventDefault();
              termRef.current?.scrollToBottom();
            }}
          >
            <ChevronsDown size={16} />
          </button>
        )}
      </div>
    </div>
  );
}
