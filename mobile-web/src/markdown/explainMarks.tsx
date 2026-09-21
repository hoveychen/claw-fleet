// Clickable `[?text]` marks — the phone's rendering half of
// shared-ts/explainMarks.ts, the counterpart of the desktop's
// claw-fleet-desktop/app/markdown/explainMarks.tsx.
//
// The remark plugin (in `mdRemarkPlugins`) turns a mark into
// `<span class="explain-mark" data-explain-quote="…">`; `ExplainMarkSpan` is the
// `span` entry of `mdComponents`, so every surface that renders markdown maps
// it. What a tap does is the surface's call, said through
// `ExplainMarksProvider`: in the transcript it asks a side question about the
// marked text on that row, in a decision card about the card's session. With
// no provider around (a wiki doc, a handoff note) the mark is plain text — there
// is no session to ask.
//
// Only agent prose is tappable: `MessageRow` stamps `data-role` on every row,
// and a mark inside a user row stays inert, mirroring `readAssistantSelection`.
import {
  createContext,
  useContext,
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
import type { ExplainAnchor } from "../generated/types";
import { t } from "../i18n";
import styles from "./explainMark.module.css";

export interface ExplainMarksContext {
  /** Ask about `quote`; `anchor` is the transcript row the mark sits in, or
   *  `undefined` when the prose is not a row (a decision card body). */
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

export function useExplainMarks(): ExplainMarksContext | null {
  return useContext(Ctx);
}

/** The row a mark sits in, as the side question's anchor; `null` for a row the
 *  user wrote (not tappable), `undefined` when the prose is not a row. */
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

export const ExplainMarkSpan: Components["span"] = function ExplainMarkSpan(props: SpanProps) {
  const { node: _node, className, children, ...rest } = props;
  const ctx = useExplainMarks();
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
  if (!ctx || !quote) return <span>{children}</span>;

  const fire = (e: MouseEvent<HTMLElement> | KeyboardEvent<HTMLElement>) => {
    const anchor = anchorOfMark(e.currentTarget);
    if (anchor === null) return;
    e.preventDefault();
    e.stopPropagation();
    ctx.onMark(quote, anchor);
  };
  return (
    <span
      className={styles.mark}
      role="button"
      tabIndex={0}
      title={t("点击解释这段话")}
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
