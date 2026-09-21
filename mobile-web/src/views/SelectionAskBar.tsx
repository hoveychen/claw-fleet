import { Languages, LoaderCircle, MessageCircleQuestion, PencilLine, Scale, X } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";

import { readAssistantSelection, type AssistantSelection } from "../../../shared-ts/sessionExplain";
import { t } from "../i18n";
import type { ExplainPreset } from "../sessionExplain";
import styles from "./SelectionAskBar.module.css";

/**
 * The floating "ask about this" bar over a long-press selection of agent
 * prose — the phone's counterpart of the desktop SelectionToolbar.
 *
 * A phone has no mouseup: the selection is made by long-pressing and
 * dragging handles, and every change fires `selectionchange`. The bar reads
 * the selection a beat after the last change (the handles fire dozens of
 * events per drag) and shows when both ends sit in one assistant row
 * (`readAssistantSelection`). It sits *below* the selection by default: the
 * system's own callout (copy / look up) takes the space above, and hiding it
 * is neither possible nor wanted.
 *
 * Tapping a button collapses the selection before the click lands, so the
 * passage is snapshotted at show time and a collapse is only acted on after a
 * short grace, cancelled when a finger is on the bar. The custom question
 * turns the bar into an input; its focus collapses the selection too, which
 * is ignored while the input is up.
 */
export function SelectionAskBar({
  scroller,
  enabled,
  busy,
  onAsk,
}: {
  /** The transcript scroller; selections are read from inside it. */
  scroller: RefObject<HTMLElement | null>;
  /** False when the session cannot be forked or another layer covers the transcript. */
  enabled: boolean;
  /** A question is being submitted; the buttons wait. */
  busy: boolean;
  onAsk: (sel: AssistantSelection, preset: ExplainPreset, question?: string) => void;
}) {
  const [shown, setShown] = useState<{ sel: AssistantSelection; rect: DOMRect } | null>(null);
  const [custom, setCustom] = useState(false);
  const [question, setQuestion] = useState("");
  const [pos, setPos] = useState<{ left: number; top: number; place: "above" | "below" } | null>(null);
  const customRef = useRef(custom);
  customRef.current = custom;
  const holding = useRef(false);
  const hideTimer = useRef<number | null>(null);
  const readTimer = useRef<number | null>(null);
  const barRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!enabled) {
      setShown(null);
      setCustom(false);
      return;
    }
    const cancelHide = () => {
      if (hideTimer.current != null) {
        window.clearTimeout(hideTimer.current);
        hideTimer.current = null;
      }
    };
    const read = () => {
      const root = scroller.current;
      if (!root) return;
      const sel = readAssistantSelection(root);
      if (sel) {
        cancelHide();
        setShown({ sel, rect: sel.rect });
        if (!customRef.current) setQuestion("");
        return;
      }
      if (customRef.current || holding.current) return;
      cancelHide();
      hideTimer.current = window.setTimeout(() => {
        hideTimer.current = null;
        if (!holding.current && !customRef.current) setShown(null);
      }, 260);
    };
    const onSelChange = () => {
      if (readTimer.current != null) window.clearTimeout(readTimer.current);
      readTimer.current = window.setTimeout(read, 120);
    };
    // The selection stays put while the page scrolls under it; follow it.
    const onScroll = () => {
      if (customRef.current) return;
      requestAnimationFrame(read);
    };
    const root = scroller.current;
    document.addEventListener("selectionchange", onSelChange);
    root?.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      document.removeEventListener("selectionchange", onSelChange);
      root?.removeEventListener("scroll", onScroll);
      cancelHide();
      if (readTimer.current != null) window.clearTimeout(readTimer.current);
    };
  }, [enabled, scroller]);

  // Place the bar once it has a width: centred under the selection, kept
  // inside the viewport, flipped above when there is no room below.
  useLayoutEffect(() => {
    if (!shown) {
      setPos(null);
      return;
    }
    const { rect } = shown;
    const width = barRef.current?.offsetWidth ?? 280;
    const height = barRef.current?.offsetHeight ?? 40;
    const margin = 8;
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    // The passage scrolled off screen: nothing to hang the bar on until it is
    // back (the scroll listener re-reads and this runs again).
    if (rect.bottom < 0 || rect.top > vh) {
      setPos(null);
      return;
    }
    const left = Math.min(Math.max(rect.left + rect.width / 2 - width / 2, margin), vw - width - margin);
    const below = rect.bottom + 10;
    // Leave the bottom strip to the composer, which floats there.
    if (below + height < vh - 96) setPos({ left, top: below, place: "below" });
    else setPos({ left, top: Math.max(rect.top - 10 - height, margin), place: "above" });
  }, [shown, custom]);

  useEffect(() => {
    if (custom) inputRef.current?.focus();
  }, [custom]);

  if (!shown) return null;
  const fire = (preset: ExplainPreset, q?: string) => {
    if (busy) return;
    onAsk(shown.sel, preset, q);
    setShown(null);
    setCustom(false);
    setQuestion("");
  };
  const dismiss = () => {
    setShown(null);
    setCustom(false);
    setQuestion("");
  };
  const hold = () => {
    holding.current = true;
  };
  const release = () => {
    // The click lands after pointerup; keep the bar through it.
    window.setTimeout(() => {
      holding.current = false;
    }, 320);
  };

  return (
    <div
      ref={barRef}
      className={styles.bar}
      data-place={pos?.place ?? "below"}
      // Unplaced (first paint, or the passage is off screen): keep it in the
      // tree for its width, out of sight.
      style={pos ? { left: pos.left, top: pos.top } : { left: 8, top: -9999, visibility: "hidden" }}
      role="toolbar"
      aria-label={t("对选中内容追问")}
      onPointerDown={hold}
      onPointerUp={release}
      onPointerCancel={release}
      data-testid="selection-ask-bar"
    >
      {custom ? (
        <form
          className={styles.custom}
          onSubmit={(e) => {
            e.preventDefault();
            const q = question.trim();
            if (q) fire("custom", q);
          }}
        >
          <input
            ref={inputRef}
            className={styles.input}
            value={question}
            onChange={(e) => setQuestion(e.target.value)}
            placeholder={t("就这段话问点什么…")}
            aria-label={t("自定义提问")}
            enterKeyHint="send"
          />
          <button type="submit" className={styles.send} disabled={busy || !question.trim()}>
            {busy ? <LoaderCircle size={14} className={styles.spin} aria-hidden="true" /> : null}
            {t("发送")}
          </button>
          <button type="button" className={styles.btn} onClick={dismiss} aria-label={t("取消")}>
            <X size={15} strokeWidth={1.9} aria-hidden="true" />
          </button>
        </form>
      ) : (
        <>
          <button type="button" className={styles.btn} onClick={() => fire("explain")} disabled={busy}>
            <MessageCircleQuestion size={14} strokeWidth={1.8} aria-hidden="true" />
            {t("解释")}
          </button>
          <button type="button" className={styles.btn} onClick={() => fire("translate")} disabled={busy}>
            <Languages size={14} strokeWidth={1.8} aria-hidden="true" />
            {t("翻译")}
          </button>
          <button type="button" className={styles.btn} onClick={() => fire("rationale")} disabled={busy}>
            <Scale size={14} strokeWidth={1.8} aria-hidden="true" />
            {t("为什么")}
          </button>
          <button
            type="button"
            className={styles.btn}
            onClick={() => setCustom(true)}
            disabled={busy}
            aria-label={t("自定义提问")}
          >
            <PencilLine size={14} strokeWidth={1.8} aria-hidden="true" />
          </button>
        </>
      )}
    </div>
  );
}
