import { Languages, MessageCircleQuestion, PencilLine, Scale, Shuffle, Swords } from "lucide-react";
import { useEffect, useRef, useState, type RefObject } from "react";
import { useTranslation } from "react-i18next";

import type { ExplainPreset } from "../explainApi";
import {
  EXPLAIN_MARK_SELECT_EVENT,
  primedSelection,
  readAssistantSelection,
  type AssistantSelection,
  type ExplainPrime,
} from "../selectionExplain";
import styles from "./SelectionToolbar.module.css";
import { Presence } from "./Presence";
import { Spinner } from "./loading";

/** Which side of the selection the bar sits on. `above` is the default; `below`
 *  when the pane has no room above (a passage on a card's first line). */
type Place = "above" | "below";
/** The bar's height as laid out (buttons 5px + 11px line + 5px, bar 3px padding
 *  and a 1px border each side), used to decide whether it fits above. */
const BAR_HEIGHT = 30;
/** Space between the bar and the selection's box. */
const BAR_GAP = 6;

/** The presets the bar offers, in order; `custom` is the pencil after them. */
const PRESETS: { preset: ExplainPreset; icon: typeof Scale; key: string; fallback: string }[] = [
  { preset: "explain", icon: MessageCircleQuestion, key: "detail.explain_preset_explain", fallback: "解释" },
  { preset: "translate", icon: Languages, key: "detail.explain_preset_translate", fallback: "翻译" },
  { preset: "rationale", icon: Scale, key: "detail.explain_preset_rationale", fallback: "为什么" },
  { preset: "steelman", icon: Swords, key: "detail.explain_preset_steelman", fallback: "反方论证" },
  { preset: "flip_conditions", icon: Shuffle, key: "detail.explain_preset_flip_conditions", fallback: "翻转条件" },
];

/** The nearest ancestor that scrolls, so a primed bar can sit at the bottom of
 *  what is visible of the pane rather than below the fold. */
function scrollParent(el: HTMLElement): HTMLElement | null {
  for (let p = el.parentElement; p; p = p.parentElement) {
    const o = getComputedStyle(p).overflowY;
    if (o === "auto" || o === "scroll") return p;
  }
  return null;
}

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
 * `prime` opens it without a selection: a decision card's "steelman" /
 * "what would flip it" buttons hand over a quote they built and a preset. The
 * bar then only *preselects* that preset — a click on a preset changes the
 * choice and nothing is sent until the reader presses the send button (or
 * submits a custom question). It sits at the bottom of the pane's visible
 * part, right above the options the buttons live next to.
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
  prime,
  primeIdx = 0,
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
  /** A card review ask to preselect; a new `nonce` re-opens the bar. */
  prime?: ExplainPrime | null;
  /** The `msgIdx` a primed ask carries (the card's question index). */
  primeIdx?: number;
}) {
  const { t } = useTranslation();
  const [shown, setShown] = useState<{
    sel: AssistantSelection;
    x: number;
    y: number;
    place: Place;
    /** Set when opened by `prime`: what the caption names. */
    caption?: string;
  } | null>(null);
  const [custom, setCustom] = useState(false);
  const [question, setQuestion] = useState("");
  // The preset a primed bar will send; `null` for a selection bar, where a
  // preset click sends at once.
  const [chosen, setChosen] = useState<ExplainPreset | null>(null);
  const chosenRef = useRef(chosen);
  chosenRef.current = chosen;
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
      setChosen(null);
      return;
    }
    const read = () => {
      const root = scroller.current;
      const host = pane.current;
      if (!root || !host) return;
      const sel = readAssistantSelection(root);
      if (!sel) {
        // A primed bar has no selection to lose: the click that opened it
        // produces a mouseup (and maybe a collapse) of its own.
        if (!customRef.current && !chosenRef.current) setShown(null);
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
      setChosen(null);
      setShown({ sel, x, y, place });
    };
    const close = () => {
      setShown(null);
      setChosen(null);
    };
    // Read after the browser has settled the selection for this gesture.
    const onUp = () => requestAnimationFrame(read);
    const onKeyUp = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        close();
        return;
      }
      if (e.shiftKey && e.key.startsWith("Arrow")) requestAnimationFrame(read);
    };
    const onSelChange = () => {
      if (customRef.current || chosenRef.current) return;
      const s = window.getSelection();
      if (!s || s.isCollapsed) setShown(null);
    };
    const onScroll = () => {
      if (!customRef.current && !chosenRef.current) setShown(null);
    };
    // A primed bar has no selection whose collapse would close it; a press
    // anywhere else does (another card button re-primes it right after).
    const onDown = (e: MouseEvent) => {
      if (!chosenRef.current || customRef.current) return;
      if (barRef.current && e.target instanceof Node && barRef.current.contains(e.target)) return;
      close();
    };
    const root = scroller.current;
    document.addEventListener("mouseup", onUp);
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keyup", onKeyUp);
    document.addEventListener("selectionchange", onSelChange);
    // A click (or Enter) on one of the agent's `[?text]` marks selects the mark
    // and announces it; read it like a mouseup, since a keyboard activation
    // has none. See markdown/explainMarks.
    document.addEventListener(EXPLAIN_MARK_SELECT_EVENT, onUp);
    root?.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      document.removeEventListener("mouseup", onUp);
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keyup", onKeyUp);
      document.removeEventListener(EXPLAIN_MARK_SELECT_EVENT, onUp);
      document.removeEventListener("selectionchange", onSelChange);
      root?.removeEventListener("scroll", onScroll);
    };
  }, [enabled, pane, scroller]);

  // Open on a card review press: preselect, never send.
  useEffect(() => {
    if (!prime || !enabled) return;
    const host = pane.current;
    if (!host) return;
    const hostRect = host.getBoundingClientRect();
    const clip = scrollParent(host)?.getBoundingClientRect();
    const bottom = clip ? Math.min(hostRect.bottom, clip.bottom) : hostRect.bottom;
    const margin = 8;
    const x = Math.min(Math.max(hostRect.width / 2, margin + 120), Math.max(hostRect.width - margin - 120, margin + 120));
    // In WebKit (the desktop app's WKWebView) the `selectionchange` this
    // removeAllRanges() causes is delivered before React commits the state
    // below. The refs must already read as primed by then, or onSelChange
    // queues a setShown(null) after ours and the bar never opens.
    chosenRef.current = prime.preset;
    customRef.current = false;
    window.getSelection()?.removeAllRanges();
    setCustom(false);
    setQuestion("");
    setChosen(prime.preset);
    setShown({
      sel: primedSelection(prime.quote, primeIdx),
      x,
      y: Math.max(bottom - hostRect.top - BAR_GAP, margin + BAR_HEIGHT),
      place: "above",
      caption: prime.caption,
    });
    // Only a new press re-opens; `primeIdx` rides along with it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [prime?.nonce, enabled]);

  useEffect(() => {
    if (custom) inputRef.current?.focus();
  }, [custom]);

  const fire = (preset: ExplainPreset, q?: string) => {
    if (busy || !shown) return;
    setPending(preset);
    onAsk(shown.sel, preset, q);
    setShown(null);
    setCustom(false);
    setChosen(null);
    setQuestion("");
  };
  // A preset button sends on a selection bar and only re-chooses on a primed one.
  const press = (preset: ExplainPreset) => {
    if (chosen) setChosen(preset);
    else fire(preset);
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
      style={{ left: shown.x, top: shown.y }}
      data-place={shown.place}
      data-primed={chosen ? "true" : undefined}
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
                setChosen(null);
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
          {shown.caption && (
            <span className={styles.caption} title={shown.caption}>
              {shown.caption}
            </span>
          )}
          {PRESETS.map(({ preset, icon: Icon, key, fallback }) => (
            <button
              key={preset}
              type="button"
              className={styles.btn}
              data-preset={preset}
              data-chosen={chosen === preset ? "true" : undefined}
              aria-pressed={chosen ? chosen === preset : undefined}
              onMouseDown={keep}
              onClick={() => press(preset)}
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
          {chosen && (
            <button
              type="button"
              className={`${styles.btn} ${styles.send}`}
              onMouseDown={keep}
              onClick={() => fire(chosen)}
              disabled={busy}
              data-testid="selection-toolbar-send"
            >
              {t("detail.explain_send", "发送")}
            </button>
          )}
        </>
      )}
    </div>
    )}</Presence>
  );
}
