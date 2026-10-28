import { Languages, MessageCircleQuestion, PencilLine, Scale, Shuffle, Swords } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";
import { useTranslation } from "react-i18next";

import type { ExplainPreset } from "../explainApi";
import {
  EXPLAIN_MARK_SELECT_EVENT,
  readAssistantSelection,
  type AssistantSelection,
} from "../selectionExplain";
import styles from "./SelectionToolbar.module.css";
import { Presence } from "./Presence";
import { Spinner } from "./loading";

/** Which side of the selection the bar sits on. `above` is the default; `below`
 *  when the pane has no room above (a passage on a card's first line). */
type Place = "above" | "below";
/** Space between the bar and the selection's box. */
const BAR_GAP = 6;
/** Space kept between the bar and the pane's side edges. */
const EDGE = 8;

/** The presets the bar offers, in order; `custom` is the pencil after them. */
const PRESETS: { preset: ExplainPreset; icon: typeof Scale; key: string; fallback: string }[] = [
  { preset: "explain", icon: MessageCircleQuestion, key: "detail.explain_preset_explain", fallback: "解释" },
  { preset: "translate", icon: Languages, key: "detail.explain_preset_translate", fallback: "翻译" },
  { preset: "rationale", icon: Scale, key: "detail.explain_preset_rationale", fallback: "为什么" },
  { preset: "steelman", icon: Swords, key: "detail.explain_preset_steelman", fallback: "反方论证" },
  { preset: "flip_conditions", icon: Shuffle, key: "detail.explain_preset_flip_conditions", fallback: "翻转条件" },
];

/**
 * The floating "ask about this" bar that appears over a selection of agent
 * prose.
 *
 * Five canned questions and a free one. It answers the moment a reader
 * finishes a drag inside an assistant row (`readAssistantSelection` decides
 * what qualifies) and goes away when the selection collapses, the pane
 * scrolls, or Escape is pressed. Choosing the custom question turns the bar
 * into an input; focusing it collapses the document selection, so the passage
 * is snapshotted at mouseup and the collapse is ignored while the input is up.
 *
 * Positioned in `pane`'s coordinate space (the pane is `position: relative`),
 * centred on the selection's box. Its width is measured once rendered: it is
 * kept within the pane, and on a pane narrower than the row of presets (a
 * decision card in a narrow dock) it wraps onto more lines instead of running
 * off the card's edges.
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
  // The selection and its box in the pane's coordinates; where the bar goes
  // (`pos`) is worked out from it once the bar has a size.
  const [shown, setShown] = useState<{
    sel: AssistantSelection;
    cx: number;
    top: number;
    bottom: number;
  } | null>(null);
  const [pos, setPos] = useState<{ left: number; top: number; maxWidth: number; place: Place } | null>(null);
  const [custom, setCustom] = useState(false);
  const [question, setQuestion] = useState("");
  // The preset whose ask is still being posted, so a bar reopened during the
  // wait shows which one is in flight instead of a row of greyed buttons.
  const [pending, setPending] = useState<ExplainPreset | null>(null);
  useEffect(() => {
    if (!busy) setPending(null);
  }, [busy]);
  const customRef = useRef(custom);
  customRef.current = custom;
  const inputRef = useRef<HTMLInputElement>(null);
  const barRef = useRef<HTMLDivElement>(null);

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
      setCustom(false);
      setQuestion("");
      setShown({
        sel,
        cx: sel.rect.left + sel.rect.width / 2 - hostRect.left,
        top: sel.rect.top - hostRect.top,
        bottom: sel.rect.bottom - hostRect.top,
      });
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

  // Place the bar once it has a size: no wider than the pane, centred on the
  // selection but kept inside the pane's edges, above the selection when the
  // pane has room for it there and below otherwise — never over the selected
  // text (a clamp to the pane's top edge used to park it right on a card's
  // first line).
  useLayoutEffect(() => {
    const host = pane.current;
    const bar = barRef.current;
    if (!shown || !host || !bar) {
      setPos(null);
      return;
    }
    const paneWidth = host.clientWidth;
    const maxWidth = Math.max(paneWidth - 2 * EDGE, 0);
    // Measure at the width it will be held to, so a wrapped bar reports the
    // height it will really have.
    bar.style.maxWidth = `${maxWidth}px`;
    const width = Math.min(bar.offsetWidth, maxWidth);
    const height = bar.offsetHeight;
    const left = Math.min(Math.max(shown.cx - width / 2, EDGE), Math.max(paneWidth - EDGE - width, EDGE));
    const above = shown.top - BAR_GAP - height;
    const place: Place = above >= EDGE ? "above" : "below";
    setPos({ left, top: place === "above" ? above : shown.bottom + BAR_GAP, maxWidth, place });
  }, [shown, custom, pane]);

  useEffect(() => {
    if (custom) inputRef.current?.focus();
  }, [custom]);

  const fire = (preset: ExplainPreset, q?: string) => {
    if (busy || !shown) return;
    setPending(preset);
    onAsk(shown.sel, preset, q);
    setShown(null);
    setCustom(false);
    setQuestion("");
  };
  // Buttons swallow mousedown so the click does not collapse the selection
  // the bar is about; the input does not, it needs the focus.
  const keep = (e: React.MouseEvent) => e.preventDefault();

  // Presence keeps the bar on screen while it fades out after the selection
  // collapses or a preset fires.
  return (
    <Presence when={!!shown}>{shown && (
    <div
      ref={barRef}
      className={styles.toolbar}
      // Unplaced (first layout): in the tree for its size, out of sight.
      style={
        pos
          ? { left: pos.left, top: pos.top, maxWidth: pos.maxWidth }
          : { left: 0, top: 0, visibility: "hidden" }
      }
      data-place={pos?.place ?? "above"}
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
            {busy ? <Spinner size={12} /> : null}
            {t("detail.explain_send", "发送")}
          </button>
        </form>
      ) : (
        <>
          {PRESETS.map(({ preset, icon: Icon, key, fallback }) => (
            <button
              key={preset}
              type="button"
              className={styles.btn}
              data-preset={preset}
              onMouseDown={keep}
              onClick={() => fire(preset)}
              disabled={busy}
            >
              {pending === preset ? <Spinner size={12} /> : <Icon size={12} strokeWidth={1.8} aria-hidden="true" />}
              {t(key, fallback)}
            </button>
          ))}
          <button
            type="button"
            className={styles.btn}
            onMouseDown={keep}
            onClick={() => setCustom(true)}
            disabled={busy}
          >
            {pending === "custom" ? (
              <Spinner size={12} />
            ) : (
              <PencilLine size={12} strokeWidth={1.8} aria-hidden="true" />
            )}
            {t("detail.explain_custom", "自定义提问")}
          </button>
        </>
      )}
    </div>
    )}</Presence>
  );
}
