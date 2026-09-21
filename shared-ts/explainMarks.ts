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
 * - A mark may span several inline siblings, so a phrase that happens to
 *   contain `` `code` ``, `**bold**` or a soft line break is still one mark
 *   (the swallowed nodes render inside the span). It is scanned over the
 *   sibling run, not inside a single text node: remark splits every bit of
 *   inline formatting into its own node, and scanning per node used to leave
 *   `[?` … `]` sitting in the prose as literal brackets — the two halves each
 *   looked unterminated. `SPANNABLE` lists what a mark may swallow.
 * - A range containing a `link` is **not** a mark and stays literal: a mark
 *   around a link would nest a clickable inside a clickable. `link` /
 *   `linkReference` subtrees are also never descended into, so a mark can
 *   never land inside link text either.
 * - Inline code and fenced code are leaves with no text children, so a mark
 *   written *inside* code is untouched.
 * - Brackets inside a mark pair up: `[?the a[0] slot]` marks `the a[0] slot`,
 *   and the `]` of an inner `[?` … `]` does not close the outer one (nesting
 *   is still unsupported — the inner `[?` stays literal text).
 * - `[?` whose `]` never arrives stays literal, as does an empty /
 *   whitespace-only mark.
 * - The span's children are the marked content verbatim; the attribute carries
 *   its flattened text, trimmed, which is what the side question quotes.
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

/** One mark located in a flat string: `[?` at `open`, its content in
 *  `[start, close)`, the closing `]` at `close`. */
interface MarkSpan {
  open: number;
  start: number;
  close: number;
}

/**
 * The next `[?` … `]` at or after `from`, with brackets paired so an inner
 * `a[0]` (or a nested `[?` … `]`) does not close the mark early. Returns null
 * once no terminated mark is left.
 */
function nextMark(text: string, from: number): MarkSpan | null {
  let at = from;
  for (;;) {
    const open = text.indexOf(OPEN, at);
    if (open < 0) return null;
    const start = open + OPEN.length;
    let depth = 0;
    let close = -1;
    for (let i = start; i < text.length; i++) {
      const ch = text[i];
      if (ch === "[") depth++;
      else if (ch === CLOSE) {
        if (depth === 0) {
          close = i;
          break;
        }
        depth--;
      }
    }
    if (close < 0) {
      // No `]` left anywhere: this `[?` and every later one is literal.
      if (text.indexOf(CLOSE, start) < 0) return null;
      // There is a `]`, but an unpaired `[` ate it. This mark is literal; a
      // later `[?` may still close cleanly.
      at = start;
      continue;
    }
    if (text.slice(start, close).trim() !== "") return { open, start, close };
    // `[?]` / `[? ]` is not a mark; keep scanning after it.
    at = close + CLOSE.length;
  }
}

/**
 * Split one text run into literal pieces and marks. Pure, so the rule set can
 * be tested without a markdown parser; `remarkExplainMarks` and
 * `stripExplainMarks` both build on it.
 */
export function splitExplainMarks(text: string): ExplainMarkPiece[] {
  const out: ExplainMarkPiece[] = [];
  let cursor = 0;
  for (;;) {
    const mark = nextMark(text, cursor);
    if (!mark) break;
    if (mark.open > cursor) out.push({ kind: "text", value: text.slice(cursor, mark.open) });
    out.push({ kind: "mark", value: text.slice(mark.start, mark.close) });
    cursor = mark.close + CLOSE.length;
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

/** Inline node types a mark may swallow when it spans more than one sibling.
 *  `link` is deliberately absent — see the module header. */
const SPANNABLE = new Set([
  "text",
  "inlineCode",
  "strong",
  "emphasis",
  "delete",
  "break",
  "inlineMath",
]);

/** One character standing in for a non-text sibling while the run is scanned.
 *  NUL is neither `[` nor `]` nor whitespace, so it can neither form a mark,
 *  close one, nor make one look empty, and markdown text never contains it. */
const OPAQUE = "\u0000";

/** A sibling's footprint in the flattened run. */
interface Unit {
  node: MdNode;
  /** Its text value when the node is a text node, else null. */
  text: string | null;
  from: number;
  to: number;
}

/** The plain text a swallowed node contributes to the quote. */
function flattenText(node: MdNode): string {
  if (node.type === "break") return "\n";
  if (typeof node.value === "string") return node.value;
  return (node.children ?? []).map(flattenText).join("");
}

function markNode(children: MdNode[]): MdNode {
  return {
    type: "explainMark",
    data: {
      hName: "span",
      hProperties: {
        className: [EXPLAIN_MARK_CLASS],
        [EXPLAIN_MARK_QUOTE_PROP]: children.map(flattenText).join("").trim(),
      },
    },
    children,
  };
}

/** Flatten a sibling run into one string plus the map back to the nodes. */
function flattenRun(children: MdNode[]): { flat: string; units: Unit[] } {
  let flat = "";
  const units: Unit[] = [];
  for (const node of children) {
    const text = node.type === "text" && typeof node.value === "string" ? node.value : null;
    const piece = text ?? OPAQUE;
    units.push({ node, text, from: flat.length, to: flat.length + piece.length });
    flat += piece;
  }
  return { flat, units };
}

/** The nodes covering `[from, to)` of the flattened run, text nodes sliced to
 *  the range. Null when the range covers a node a mark may not swallow. */
function sliceRun(units: Unit[], from: number, to: number): MdNode[] | null {
  const out: MdNode[] = [];
  for (const u of units) {
    if (u.to <= from || u.from >= to) continue;
    if (u.text === null) {
      if (!SPANNABLE.has(u.node.type)) return null;
      out.push(u.node);
      continue;
    }
    const value = u.text.slice(Math.max(from, u.from) - u.from, Math.min(to, u.to) - u.from);
    if (value) out.push({ type: "text", value });
  }
  return out.length ? out : null;
}

function walk(node: MdNode): void {
  const children = node.children;
  if (!children) return;
  const { flat, units } = flattenRun(children);
  const out: MdNode[] = [];
  let cursor = 0;
  if (flat.includes(OPEN)) {
    for (;;) {
      const mark = nextMark(flat, cursor);
      if (!mark) break;
      // A mark's brackets each have to sit inside one text node: with `[?`
      // split across a node boundary the halves are not adjacent anyway.
      const inner = sliceRun(units, mark.start, mark.close);
      if (!inner) {
        // Not markable (a link in the range, say) — leave it as written and
        // look for the next mark after it.
        cursor = mark.close + CLOSE.length;
        continue;
      }
      const before = sliceRun(units, cursor, mark.open);
      if (before) out.push(...before);
      out.push(markNode(inner));
      cursor = mark.close + CLOSE.length;
    }
  }
  if (out.length) {
    const tail = sliceRun(units, cursor, flat.length);
    if (tail) out.push(...tail);
    node.children = out;
  }
  // Descend into the siblings that stayed whole. A mark's own children are
  // left alone: nesting is unsupported, so a `[?` inside one stays literal.
  for (const child of node.children ?? []) {
    if (child.type === "explainMark" || SKIP.has(child.type)) continue;
    walk(child);
  }
}

/** remark plugin: `[?text]` in a run of inline nodes → an `explainMark` node that
 *  remark-rehype renders as `<span class="explain-mark" data-explain-quote>`. */
export function remarkExplainMarks() {
  return (tree: unknown) => {
    walk(tree as MdNode);
  };
}
