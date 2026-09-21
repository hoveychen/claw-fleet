// Clickable `[?text]` marks — the phone's rendering half of
// shared-ts/explainMarks.ts, the counterpart of the desktop's
// claw-fleet-desktop/app/markdown/explainMarks.tsx.
//
// The remark plugin (in `mdRemarkPlugins`) turns a mark into
// `<span class="explain-mark" data-explain-quote="…">`; `ExplainMarkSpan` is the
// `span` entry of `mdComponents`, so every surface that renders markdown maps
// it. A tap does not ask anything by itself: it *selects* the marked text, the
// way a long-press would, and the `SelectionAskBar` that owns the surrounding
// prose (a transcript row, or a decision card's question) reads the selection
// and offers 解释 / 翻译 / 为什么 / a custom question. One more tap on 解释 is the
// no-typing path.
//
// Which marks are tappable follows the bar's own rule (`readAssistantSelection`):
// the mark has to sit inside an `[data-msg-idx][data-role="assistant"]`
// container. A mark in the user's words, a wiki doc or a side question's answer
// has no bar to speak to and renders as plain text.
import {
  useLayoutEffect,
  useRef,
  useState,
  type ComponentPropsWithoutRef,
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
} from "react";
import type { Components } from "react-markdown";

import {
  EXPLAIN_MARK_CLASS,
  EXPLAIN_MARK_QUOTE_ATTR,
  EXPLAIN_MARK_QUOTE_PROP,
} from "../../../shared-ts/explainMarks";
import { selectExplainMark } from "../../../shared-ts/sessionExplain";
import { t } from "../i18n";
import styles from "./explainMark.module.css";

export const ASSISTANT_PROSE_SELECTOR = "[data-msg-idx][data-role='assistant']";

/** react-markdown hands each component the DOM props plus the hast `node`,
 *  which must not reach the element (`node="[object Object]"`). */
type SpanProps = ComponentPropsWithoutRef<"span"> & { node?: unknown };

export const ExplainMarkSpan: Components["span"] = function ExplainMarkSpan(props: SpanProps) {
  const { node: _node, className, children, ...rest } = props;
  const classes = typeof className === "string" ? className.split(/\s+/) : [];
  if (!classes.includes(EXPLAIN_MARK_CLASS)) {
    return (
      <span className={className} {...rest}>
        {children}
      </span>
    );
  }
  const bag = rest as Record<string, unknown>;
  const raw = bag[EXPLAIN_MARK_QUOTE_ATTR] ?? bag[EXPLAIN_MARK_QUOTE_PROP];
  const quote = typeof raw === "string" && raw.trim() ? raw : null;
  return <ExplainMark quote={quote}>{children}</ExplainMark>;
};

function ExplainMark({ quote, children }: { quote: string | null; children?: ReactNode }) {
  const ref = useRef<HTMLSpanElement>(null);
  // Decided from the DOM once mounted — the same containment test the ask bar
  // runs on a selection, so a mark is tappable exactly when a tap leads somewhere.
  const [clickable, setClickable] = useState(false);
  useLayoutEffect(() => {
    setClickable(!!quote && !!ref.current?.closest(ASSISTANT_PROSE_SELECTOR));
  }, [quote]);

  if (!clickable) {
    return (
      <span ref={ref} data-explain-quote={quote ?? undefined}>
        {children}
      </span>
    );
  }
  const fire = (e: MouseEvent<HTMLElement> | KeyboardEvent<HTMLElement>) => {
    e.preventDefault();
    e.stopPropagation();
    selectExplainMark(e.currentTarget);
  };
  return (
    <span
      ref={ref}
      className={styles.mark}
      role="button"
      tabIndex={0}
      title={t("点击对这段话追问")}
      data-explain-quote={quote ?? undefined}
      onClick={fire}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") fire(e);
      }}
    >
      {children}
    </span>
  );
}
