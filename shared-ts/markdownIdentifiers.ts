/**
 * Keeping underscores that belong to identifiers when a card previews what the
 * boss last said.
 *
 * Markdown reads `_` as an emphasis delimiter, and two places in the preview
 * took that too literally:
 *
 *   - The collapsed one-line snippet stripped every `_`, so `test_hello.py`
 *     became `testhello.py` and `fleet__spawn` became `fleetspawn`.
 *   - The expanded body renders through remark, which (correctly, per
 *     CommonMark) leaves intraword underscores alone but turns a bare
 *     `__pycache__` / `__init__` into bold `pycache` / `init`.
 *
 * Both helpers here keep emphasis working — `_word_`, `**bold**`,
 * `__bold text__` still lose their markers — and only spare the shapes that
 * are names: underscores inside a word, and a single-word `__dunder__`.
 *
 * Dependency-free on purpose: this file lives outside both packages'
 * `node_modules`, so it cannot import `unist-util-visit` or mdast types.
 */

/** Letter, digit or underscore — a character that can sit inside a name. */
const WORD = String.raw`[\p{L}\p{N}_]`;

/**
 * An underscore emphasis span: a 1–3 underscore run that does not touch a
 * word character on its outer side, wrapping text that does not start or end
 * with whitespace, closed by the same run. Mirrors CommonMark's rule that `_`
 * cannot open or close emphasis inside a word.
 */
const UNDERSCORE_SPAN = new RegExp(
  String.raw`(^|[^\p{L}\p{N}_\\])(_{1,3})(?![\s_])(.+?)(?<![\s_\\])\2(?!${WORD})`,
  "gu",
);

const IDENTIFIER = new RegExp(String.raw`^${WORD}+$`, "u");

/** `__name__`: a double-underscore pair around one bare word reads as a
 *  Python-style dunder name far more often than as bold. */
function isDunder(run: string, inner: string): boolean {
  return run === "__" && IDENTIFIER.test(inner);
}

/**
 * Drop markdown emphasis markers from plain text without eating identifier
 * underscores. `*` is always a marker here; `_` only where it opens and closes
 * an emphasis span. Underscores that cannot be delimiters (`test_hello`,
 * `fleet__spawn`, an unmatched `_private`) and `__dunder__` names stay.
 */
export function stripEmphasisMarkers(text: string): string {
  return text
    .replace(/\*+/g, "")
    .replace(UNDERSCORE_SPAN, (whole, lead: string, run: string, inner: string) =>
      isDunder(run, inner) ? whole : `${lead}${inner}`,
    );
}

interface MdNode {
  type: string;
  value?: string;
  children?: MdNode[];
  position?: { start: { offset?: number }; end: { offset?: number } };
}

/**
 * remark plugin: turn a `strong` node that came from a single-word
 * `__dunder__` back into the literal text it was written as. `**word**` and
 * multi-word `__bold text__` stay bold — the delimiter is read off the source,
 * which is why this needs the node positions remark records by default.
 */
export function remarkKeepDunder() {
  return (tree: MdNode, file: { value?: unknown; toString(): string }) => {
    const source = typeof file.value === "string" ? file.value : String(file);
    const walk = (node: MdNode) => {
      const kids = node.children;
      if (!kids) return;
      for (let i = 0; i < kids.length; i++) {
        const k = kids[i];
        const literal = dunderLiteral(k, source);
        if (literal !== null) {
          kids[i] = { type: "text", value: literal, position: k.position };
          continue;
        }
        walk(k);
      }
    };
    walk(tree);
  };
}

function dunderLiteral(node: MdNode, source: string): string | null {
  if (node.type !== "strong") return null;
  const start = node.position?.start.offset;
  const end = node.position?.end.offset;
  if (start === undefined || end === undefined) return null;
  const raw = source.slice(start, end);
  if (!raw.startsWith("__") || !raw.endsWith("__")) return null;
  return isDunder("__", raw.slice(2, -2)) ? raw : null;
}
