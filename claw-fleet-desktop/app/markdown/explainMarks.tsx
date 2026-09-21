import {
  useLayoutEffect,
  useRef,
  useState,
  type ComponentPropsWithoutRef,
  type KeyboardEvent,
  type MouseEvent,
} from "react";
import type { Components } from "react-markdown";
import { useTranslation } from "react-i18next";

import { EXPLAIN_MARK_ATTR_QUOTE_PROPS, EXPLAIN_MARK_CLASS } from "./explainMarkProps";
import { selectExplainMark } from "../selectionExplain";
import styles from "./markdown.module.css";

/**
 * Clickable `[?text]` marks — the rendering half of shared-ts/explainMarks.ts.
 *
 * The remark plugin (in `safeRemarkPlugins`) turns a mark into
 * `<span class="explain-mark" data-explain-quote="…">`; this is the `span`
 * component every markdown surface maps that element to.
 *
 * A click does not ask anything by itself: it *selects* the marked text, the
 * way a drag would, and the ask bar that owns the surrounding prose
 * (`SelectionToolbar`, over a transcript row or a decision card's question)
 * reads the selection and offers 解释 / 翻译 / 为什么 / a custom question. One
 * more click on 解释 is the no-typing path.
 *
 * Which marks are clickable follows the same rule the bar itself applies
 * (`readAssistantSelection`): the mark has to sit inside an
 * `[data-msg-idx][data-role="assistant"]` container. A mark in the user's own
 * words, in a wiki doc, or in a side question's answer has no bar to speak to
 * and renders as plain text.
 */
export const ASSISTANT_PROSE_SELECTOR = "[data-msg-idx][data-role='assistant']";

/** react-markdown hands each component the DOM props plus the hast `node`,
 *  which must not reach the element (`node="[object Object]"`). */
type SpanProps = ComponentPropsWithoutRef<"span"> & { node?: unknown };

/** The `span` renderer: an explain mark becomes a clickable annotation, any
 *  other span (KaTeX's, a raw one) passes through untouched. */
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
  return <ExplainMark quote={readQuote(rest as Record<string, unknown>)}>{children}</ExplainMark>;
};

function ExplainMark({ quote, children }: { quote: string | null; children?: React.ReactNode }) {
  const { t } = useTranslation();
  const ref = useRef<HTMLSpanElement>(null);
  // Decided from the DOM, once mounted: the same containment test the ask bar
  // runs on a selection, so a mark is clickable exactly when a click can lead
  // somewhere.
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
      className={styles.explain_mark}
      role="button"
      tabIndex={0}
      title={t("markdown.explain_mark_hint", "点击对这段话追问")}
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

function readQuote(props: Record<string, unknown>): string | null {
  for (const key of EXPLAIN_MARK_ATTR_QUOTE_PROPS) {
    const v = props[key];
    if (typeof v === "string" && v.trim()) return v;
  }
  return null;
}
