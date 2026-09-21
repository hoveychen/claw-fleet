import {
  createContext,
  useContext,
  type ComponentPropsWithoutRef,
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
} from "react";
import type { Components } from "react-markdown";
import { useTranslation } from "react-i18next";

import { EXPLAIN_MARK_ATTR_QUOTE_PROPS, EXPLAIN_MARK_CLASS } from "./explainMarkProps";
import type { ExplainAnchor } from "../explainApi";
import styles from "./markdown.module.css";

/**
 * Clickable `[?text]` marks — the rendering half of shared-ts/explainMarks.ts.
 *
 * The remark plugin (in `safeRemarkPlugins`) turns a mark into
 * `<span class="explain-mark" data-explain-quote="…">`; this file is the `span`
 * component every markdown surface maps that element to. What a click *does*
 * depends on where the prose is: in a transcript it asks a side question about
 * the marked text on that row, in a decision card it asks about the card's
 * session. The surface says so by providing `ExplainMarksProvider`; outside any
 * provider (a wiki doc, a handoff note) the mark renders as plain text with no
 * affordance, since there is no session to ask.
 *
 * Only agent prose is clickable. The transcript stamps every row with
 * `data-role`, and a mark inside a `user` row — the person quoting the agent
 * back, say — stays inert, mirroring `readAssistantSelection`.
 */
export interface ExplainMarksContext {
  /**
   * Ask about `quote`. `anchor` is the transcript row the mark sits in
   * (`data-msg-uuid` / `data-msg-idx`), or `undefined` when the prose is not a
   * transcript row (a decision card body).
   */
  onMark: (quote: string, anchor: ExplainAnchor | undefined) => void;
}

const Ctx = createContext<ExplainMarksContext | null>(null);

export function ExplainMarksProvider({
  value,
  children,
}: {
  value: ExplainMarksContext | null;
  children: ReactNode;
}) {
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

/** The ambient handler, or `null` outside any provider. */
export function useExplainMarks(): ExplainMarksContext | null {
  return useContext(Ctx);
}

/** The row a mark sits in, as the side question's anchor; `null` for a row the
 *  user wrote (not clickable), `undefined` when the prose is not a row. */
export function anchorOfMark(el: Element): ExplainAnchor | null | undefined {
  const row = el.closest<HTMLElement>("[data-msg-idx][data-role]");
  if (!row) return undefined;
  if (row.getAttribute("data-role") !== "assistant") return null;
  const idx = Number(row.getAttribute("data-msg-idx"));
  return {
    msgUuid: row.getAttribute("data-msg-uuid") ?? undefined,
    msgIdx: Number.isFinite(idx) ? idx : undefined,
  };
}

/** react-markdown hands each component the DOM props plus the hast `node`,
 *  which must not reach the element (`node="[object Object]"`). */
type SpanProps = ComponentPropsWithoutRef<"span"> & { node?: unknown };

/** The `span` renderer: an explain mark becomes a clickable annotation, any
 *  other span (KaTeX's, a raw one) passes through untouched. */
export const ExplainMarkSpan: Components["span"] = function ExplainMarkSpan(props: SpanProps) {
  const { node: _node, className, children, ...rest } = props;
  const ctx = useExplainMarks();
  const { t } = useTranslation();
  const classes = typeof className === "string" ? className.split(/\s+/) : [];
  if (!classes.includes(EXPLAIN_MARK_CLASS)) {
    return (
      <span className={className} {...rest}>
        {children}
      </span>
    );
  }
  const quote = readQuote(rest as Record<string, unknown>);
  // No provider, or the plugin found nothing to quote: the text, unadorned.
  if (!ctx || !quote) return <span>{children}</span>;

  const fire = (e: MouseEvent<HTMLElement> | KeyboardEvent<HTMLElement>) => {
    const anchor = anchorOfMark(e.currentTarget);
    if (anchor === null) return; // the user's own words
    e.preventDefault();
    e.stopPropagation();
    ctx.onMark(quote, anchor);
  };
  return (
    <span
      className={styles.explain_mark}
      role="button"
      tabIndex={0}
      title={t("markdown.explain_mark_hint", "点击解释这段话")}
      data-explain-quote={quote}
      onClick={fire}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") fire(e);
      }}
    >
      {children}
    </span>
  );
};

function readQuote(props: Record<string, unknown>): string | null {
  for (const key of EXPLAIN_MARK_ATTR_QUOTE_PROPS) {
    const v = props[key];
    if (typeof v === "string" && v.trim()) return v;
  }
  return null;
}
