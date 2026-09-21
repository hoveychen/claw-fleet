/**
 * Inline explain marks — `[?text]` in agent prose.
 *
 * The agent wraps a phrase or sentence it suspects is under-explained in
 * `[?` … `]` (see `design/explain-annotations.md`). Anywhere else the text is
 * read — a terminal, a plain markdown viewer — that is a harmless bracketed
 * aside; on Fleet's surfaces it becomes a clickable annotation that asks the
 * v1 side-question machinery to explain exactly that text.
 *
 * This module is the parsing half, shared by the desktop and the phone (both
 * `tsconfig`s include `../shared-ts`, so it must not import anything either
 * package lacks — the mdast shapes it needs are declared structurally below).
 * `remarkExplainMarks` rewrites text nodes in the mdast, so a mark survives
 * remark-rehype as a `<span class="explain-mark" data-explain-quote="…">`
 * without going through rehype-raw; each package's sanitize schema admits that
 * class and attribute (their `plugins.test.ts` pins it). Rendering the span as
 * something clickable is each surface's job.
 *
 * Rules:
 * - Only text nodes are rewritten. Inline code and fenced code are leaves with
 *   no text children, so they are naturally untouched; `link` /
 *   `linkReference` subtrees are skipped explicitly so a mark never lands
 *   inside link text.
 * - A mark is the shortest `[?` … `]`: nesting is not supported, an inner `[?`
 *   is literal text inside the outer mark.
 * - `[?` with no closing `]` in the same text node stays literal, as does an
 *   empty / whitespace-only mark.
 * - The span's children are the marked text verbatim; the attribute carries it
 *   trimmed, which is what the side question quotes.
 */

/** The class the plugin puts on the emitted span; surfaces key their component map on it. */
export const EXPLAIN_MARK_CLASS = "explain-mark";
/** hast property name of the quote attribute (`data-explain-quote` in the DOM). */
export const EXPLAIN_MARK_QUOTE_PROP = "dataExplainQuote";
/** The DOM attribute the quote lands in. */
export const EXPLAIN_MARK_QUOTE_ATTR = "data-explain-quote";

const OPEN = "[?";
const CLOSE = "]";

/** A piece of a text run after mark extraction. */
export type ExplainMarkPiece = { kind: "text"; value: string } | { kind: "mark"; value: string };

/**
 * Split one text run into literal pieces and marks. Pure, so the rule set can
 * be tested without a markdown parser; `remarkExplainMarks` and
 * `stripExplainMarks` both build on it.
 */
export function splitExplainMarks(text: string): ExplainMarkPiece[] {
  const out: ExplainMarkPiece[] = [];
  let cursor = 0;
  let from = 0;
  for (;;) {
    const open = text.indexOf(OPEN, from);
    if (open < 0) break;
    const close = text.indexOf(CLOSE, open + OPEN.length);
    if (close < 0) break; // unterminated: everything from here is literal
    const inner = text.slice(open + OPEN.length, close);
    if (inner.trim() === "") {
      // `[?]` / `[? ]` is not a mark; keep scanning after it.
      from = close + CLOSE.length;
      continue;
    }
    if (open > cursor) out.push({ kind: "text", value: text.slice(cursor, open) });
    out.push({ kind: "mark", value: inner });
    cursor = close + CLOSE.length;
    from = cursor;
  }
  if (cursor < text.length) out.push({ kind: "text", value: text.slice(cursor) });
  return out;
}

/** `[?text]` → `text`, for surfaces that speak or preview prose as plain text
 *  (TTS, one-line summaries). String-level: it does not know about code
 *  fences, which is fine for those surfaces and wrong for anything rendered —
 *  rendered surfaces use the remark plugin. */
export function stripExplainMarks(md: string): string {
  if (!md.includes(OPEN)) return md;
  return splitExplainMarks(md)
    .map((p) => p.value)
    .join("");
}

// ── remark plugin ────────────────────────────────────────────────────────────

/** The slice of mdast this plugin reads and writes. */
interface MdNode {
  type: string;
  value?: string;
  children?: MdNode[];
  data?: Record<string, unknown>;
}

/** Parents whose text must never be rewritten (their text is a link's label). */
const SKIP = new Set(["link", "linkReference"]);

function markNode(inner: string): MdNode {
  return {
    type: "explainMark",
    data: {
      hName: "span",
      hProperties: {
        className: [EXPLAIN_MARK_CLASS],
        [EXPLAIN_MARK_QUOTE_PROP]: inner.trim(),
      },
    },
    children: [{ type: "text", value: inner }],
  };
}

function walk(node: MdNode): void {
  const children = node.children;
  if (!children) return;
  let changed = false;
  const out: MdNode[] = [];
  for (const child of children) {
    if (child.type !== "text" || !child.value || !child.value.includes(OPEN)) {
      if (!SKIP.has(child.type)) walk(child);
      out.push(child);
      continue;
    }
    const pieces = splitExplainMarks(child.value);
    if (pieces.length === 1 && pieces[0].kind === "text") {
      out.push(child);
      continue;
    }
    changed = true;
    for (const p of pieces) {
      out.push(p.kind === "mark" ? markNode(p.value) : { type: "text", value: p.value });
    }
  }
  if (changed) node.children = out;
}

/** remark plugin: `[?text]` in text nodes → an `explainMark` node that
 *  remark-rehype renders as `<span class="explain-mark" data-explain-quote>`. */
export function remarkExplainMarks() {
  return (tree: unknown) => {
    walk(tree as MdNode);
  };
}
