/**
 * Split a markdown source into chunks that can each be parsed and rendered on
 * their own, so a long document reaches the screen a screenful at a time
 * instead of after one multi-second parse of the whole thing.
 *
 * The hard contract is `chunkMarkdown(src).join("") === src`: this is a
 * *rendering* strategy, never a content one. Nothing is dropped, summarised or
 * elided — the reader still gets every byte, just not all in the first frame.
 *
 * Chunks are only cut at boundaries where splitting cannot change how the
 * markdown reads, because each chunk is parsed by its own processor and knows
 * nothing about its neighbours: a cut inside a fenced block would turn the rest
 * of the document into code, a cut inside a table would leave two half-tables,
 * a cut inside an ordered list would restart its numbering at 1.
 */

/** Rough size a chunk aims for. Small enough that one chunk parses in tens of
 *  milliseconds through the app's remark/rehype chain, large enough that a
 *  normal document stays a single chunk. */
export const DEFAULT_CHUNK_BYTES = 32 * 1024;

/** What a non-blank line looks like structurally — only the distinctions that
 *  decide whether a cut next to it is safe. */
type LineKind = "ordered" | "indented" | "other";

function classify(line: string): LineKind {
  // `1.` / `1)` — a cut here would restart the numbering in the next chunk.
  if (/^\s*\d+[.)]\s/.test(line)) return "ordered";
  // Four-space indent: either an indented code block or a nested list item's
  // continuation. Both belong with what precedes them.
  if (/^\s{4,}\S/.test(line)) return "indented";
  return "other";
}

/** Kinds that must not be separated from what precedes them: each continues a
 *  block whose opening lives in the previous chunk, and a freshly-parsed chunk
 *  has no way to know that.
 *
 *  Two absences are deliberate, both established by probing what remark
 *  actually produces rather than by guessing:
 *
 *  - A bullet list (`-` / `*`) classifies as "other", so a cut between two
 *    bullets is allowed. Splitting one `<ul>` into two is invisible (every
 *    bullet is the same glyph), and forbidding it would leave a document that
 *    is one long bullet list — which `TASKS.md` very nearly is — with no legal
 *    cut anywhere.
 *  - A blockquote is absent because blank-separated quotes are *already* two
 *    blockquotes: `> a\n\n> b` parses to two `<blockquote>` elements, so a cut
 *    there changes nothing. A quote that really continues across a line break
 *    uses `>` on its own line, which is not blank and so is never a cut point
 *    to begin with. A GFM table is absent for the same kind of reason: a blank
 *    line *terminates* a table (the rows after it parse as a paragraph), so no
 *    table can ever span a cut point. An ordered list is the opposite case —
 *    blank-separated items stay one `<ol>` — which is why it is listed. */
const UNCUTTABLE = new Set<LineKind>(["ordered", "indented"]);

function stripEol(line: string): string {
  return line.endsWith("\n") ? line.slice(0, -1) : line;
}

/** Index of the first non-blank line at or after `i`, or -1. */
function nextNonBlank(lines: string[], i: number): number {
  for (let j = i; j < lines.length; j += 1) if (lines[j].trim() !== "") return j;
  return -1;
}

/**
 * May a chunk end after line `i`? Only at a blank line that is not followed by
 * the continuation of a block which started before it.
 *
 * Only the line *after* the cut is examined. A symmetric check on the line
 * before it was written first and then removed: every case where the preceding
 * line is a continuation is a case where the following one is too (both halves
 * of a table, of a loose ordered list, of an indented block), so the backward
 * look could not be made to fail any test — an unfalsifiable rule that only
 * cost legal cut points.
 */
function canBreakAfter(lines: string[], i: number): boolean {
  if (lines[i].trim() !== "") return false;
  const next = nextNonBlank(lines, i + 1);
  if (next === -1) return false; // trailing blanks — nothing left to defer
  return !UNCUTTABLE.has(classify(stripEol(lines[next])));
}

/**
 * Cut `src` into renderable chunks. Concatenating the result reproduces `src`
 * byte for byte; a document with no legal cut point comes back as one chunk.
 */
export function chunkMarkdown(src: string, targetBytes = DEFAULT_CHUNK_BYTES): string[] {
  if (!src) return [];
  // Lookbehind keeps each line's own newline attached, which is what makes the
  // join-back exact — `split("\n")` would drop the information about whether
  // the source ended with one.
  const lines = src.split(/(?<=\n)/);

  const chunks: string[] = [];
  let start = 0;
  let size = 0;
  let fence: string | null = null;

  for (let i = 0; i < lines.length; i += 1) {
    const line = stripEol(lines[i]);
    const trimmed = line.trim();

    const fenceMatch = /^(```+|~~~+)/.exec(trimmed);
    if (fenceMatch) {
      if (fence && trimmed.startsWith(fence)) fence = null;
      else if (!fence) fence = fenceMatch[1];
    }

    size += lines[i].length;
    // Inside a fence there is no legal cut at all, however large the block —
    // an unterminated fence would swallow every chunk after it.
    if (size >= targetBytes && fence === null && canBreakAfter(lines, i)) {
      chunks.push(lines.slice(start, i + 1).join(""));
      start = i + 1;
      size = 0;
    }
  }

  if (start < lines.length) chunks.push(lines.slice(start).join(""));
  return chunks;
}
