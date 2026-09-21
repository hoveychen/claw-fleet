import { Languages, LoaderCircle, MessageCircleQuestion, PencilLine, Scale } from "lucide-react";
import { useEffect, useRef, useState, type RefObject } from "react";
import { useTranslation } from "react-i18next";

import type { ExplainPreset } from "../explainApi";
import {
  EXPLAIN_MARK_SELECT_EVENT,
  readAssistantSelection,
  type AssistantSelection,
} from "../selectionExplain";
import styles from "./SelectionToolbar.module.css";

/** Which side of the selection the bar sits on. `above` is the default; `below`
 *  when the pane has no room above (a passage on a card's first line). */
type Place = "above" | "below";
/** The bar's height as laid out (buttons 5px + 11px line + 5px, bar 3px padding
 *  and a 1px border each side), used to decide whether it fits above. */
const BAR_HEIGHT = 30;
/** Space between the bar and the selection's box. */
const BAR_GAP = 6;

/**
 * The floating "ask about this" bar that appears over a selection of agent
 * prose.
 *
 * Three canned questions and a free one. It answers the moment a reader
 * finishes a drag inside an assistant row (`readAssistantSelection` decides
 * what qualifies) and goes away when the selection collapses, the pane
 * scrolls, or Escape is pressed. Choosing the custom question turns the bar
 * into an input; focusing it collapses the document selection, so the passage
 * is snapshotted at mouseup and the collapse is ignored while the input is up.
 *
 * Positioned in `pane`'s coordinate space (the pane is `position: relative`),
 * centred above the selection's box and clamped to the pane's width.
 */
export function SelectionToolbar({
  pane,
  scroller,
  enabled,
  busy,
  onAsk,
}: {
  /** The element the bar is positioned inside. */
  pane: RefObject<HTMLElement | null>;
  /** The element selections are read from inside of (`readAssistantSelection`
   *  needs them in an `[data-msg-idx][data-role=assistant]` container in it);
   *  a scroll on it dismisses the bar. The transcript scroller, or a decision
   *  card's question body. */
  scroller: RefObject<HTMLElement | null>;
  /** False when the session cannot be forked (no transcript path yet). */
  enabled: boolean;
  /** A question is being submitted; the buttons wait. */
  busy: boolean;
  onAsk: (sel: AssistantSelection, preset: ExplainPreset, question?: string) => void;
}) {
  const { t } = useTranslation();
  const [shown, setShown] = useState<{ sel: AssistantSelection; x: number; y: number; place: Place } | null>(
    null,
  );
  const [custom, setCustom] = useState(false);
  const [question, setQuestion] = useState("");
  const customRef = useRef(custom);
  customRef.current = custom;
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!enabled) {
      setShown(null);
      return;
    }
    const read = () => {
      const root = scroller.current;
      const host = pane.current;
      if (!root || !host) return;
      const sel = readAssistantSelection(root);
      if (!sel) {
        if (!customRef.current) setShown(null);
        return;
      }
      const hostRect = host.getBoundingClientRect();
      const margin = 8;
      const x = Math.min(
        Math.max(sel.rect.left + sel.rect.width / 2 - hostRect.left, margin + 120),
        hostRect.width - margin - 120,
      );
      // Above the selection when the pane has room for the bar there;
      // otherwise below it. Never over the selected text: a clamp used to
      // park the bar at the pane's top edge, which on a decision card's
      // first line meant right on top of the passage being asked about.
      const above = sel.rect.top - hostRect.top - BAR_GAP;
      const place: Place = above >= margin + BAR_HEIGHT ? "above" : "below";
      const y = place === "above" ? above : sel.rect.bottom - hostRect.top + BAR_GAP;
      setCustom(false);
      setQuestion("");
      setShown({ sel, x, y, place });
    };
    // Read after the browser has settled the selection for this gesture.
    const onUp = () => requestAnimationFrame(read);
    const onKeyUp = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        setShown(null);
        return;
      }
      if (e.shiftKey && e.key.startsWith("Arrow")) requestAnimationFrame(read);
    };
    const onSelChange = () => {
      if (customRef.current) return;
      const s = window.getSelection();
      if (!s || s.isCollapsed) setShown(null);
    };
    const onScroll = () => {
      if (!customRef.current) setShown(null);
    };
    const root = scroller.current;
    document.addEventListener("mouseup", onUp);
    document.addEventListener("keyup", onKeyUp);
    document.addEventListener("selectionchange", onSelChange);
    // A click (or Enter) on one of the agent's `[?text]` marks selects the mark
    // and announces it; read it like a mouseup, since a keyboard activation
    // has none. See markdown/explainMarks.
    document.addEventListener(EXPLAIN_MARK_SELECT_EVENT, onUp);
    root?.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      document.removeEventListener("mouseup", onUp);
      document.removeEventListener("keyup", onKeyUp);
      document.removeEventListener(EXPLAIN_MARK_SELECT_EVENT, onUp);
      document.removeEventListener("selectionchange", onSelChange);
      root?.removeEventListener("scroll", onScroll);
    };
  }, [enabled, pane, scroller]);

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
  // Buttons swallow mousedown so the click does not collapse the selection
  // the bar is about; the input does not, it needs the focus.
  const keep = (e: React.MouseEvent) => e.preventDefault();

  return (
    <div
      className={styles.toolbar}
      style={{ left: shown.x, top: shown.y }}
      data-place={shown.place}
      role="toolbar"
      aria-label={t("detail.explain_toolbar", "对选中内容追问")}
      data-testid="selection-toolbar"
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
            onKeyDown={(e) => {
              if (e.key === "Escape") {
                e.stopPropagation();
                setShown(null);
                setCustom(false);
              }
            }}
            placeholder={t("detail.explain_custom_placeholder", "就这段话问点什么…")}
            aria-label={t("detail.explain_custom", "自定义提问")}
          />
          <button type="submit" className={styles.btn} disabled={busy || !question.trim()}>
            {busy ? <LoaderCircle size={12} className={styles.spin} aria-hidden="true" /> : null}
            {t("detail.explain_send", "发送")}
          </button>
        </form>
      ) : (
        <>
          <button type="button" className={styles.btn} onMouseDown={keep} onClick={() => fire("explain")} disabled={busy}>
            <MessageCircleQuestion size={12} strokeWidth={1.8} aria-hidden="true" />
            {t("detail.explain_preset_explain", "解释")}
          </button>
          <button type="button" className={styles.btn} onMouseDown={keep} onClick={() => fire("translate")} disabled={busy}>
            <Languages size={12} strokeWidth={1.8} aria-hidden="true" />
            {t("detail.explain_preset_translate", "翻译")}
          </button>
          <button type="button" className={styles.btn} onMouseDown={keep} onClick={() => fire("rationale")} disabled={busy}>
            <Scale size={12} strokeWidth={1.8} aria-hidden="true" />
            {t("detail.explain_preset_rationale", "为什么")}
          </button>
          <button
            type="button"
            className={styles.btn}
            onMouseDown={keep}
            onClick={() => setCustom(true)}
            disabled={busy}
          >
            <PencilLine size={12} strokeWidth={1.8} aria-hidden="true" />
            {t("detail.explain_custom", "自定义提问")}
          </button>
        </>
      )}
    </div>
  );
}
