/**
 * Selecting agent prose to ask about it — the DOM half of "selection explain".
 *
 * The transcript renders every message row with a `data-msg-idx`, and assistant
 * rows additionally carry `data-role="assistant"` (see MessageList). A
 * selection qualifies when it is non-empty and *both* ends sit inside one
 * assistant row: a drag that starts in the user's bubble or spans two turns is
 * not a passage the agent wrote, and the fork prompt quotes it as one.
 *
 * Pure DOM helpers, no React, so the rules can be tested in jsdom without
 * mounting the transcript.
 */

import type { ExplainRecord } from "./generated/types";

export interface AssistantSelection {
  /** The selected text, verbatim (trimmed). */
  quote: string;
  /** The row's `data-msg-idx`, an index into the transcript as rendered. */
  msgIdx: number;
  /** The row's `data-msg-uuid`, when the transcript record had one. */
  msgUuid: string | null;
  /** The selection's bounding box in viewport coordinates. */
  rect: DOMRect;
}

/** Shortest selection worth a fork: a single glyph is a mis-drag. */
export const MIN_QUOTE_CHARS = 2;
/** Longest quote we will put in a prompt; the fork has the whole transcript
 *  anyway, the quote is only there to point at the passage. */
export const MAX_QUOTE_CHARS = 4000;

function rowOf(node: Node | null): HTMLElement | null {
  const el = node instanceof Element ? node : node?.parentElement ?? null;
  return el?.closest?.("[data-msg-idx][data-role='assistant']") ?? null;
}

/**
 * The current selection, if it is a quotable passage of agent prose inside
 * `root`. `null` for everything else: collapsed, outside the transcript, not
 * assistant prose, or straddling two rows.
 */
export function readAssistantSelection(
  root: HTMLElement,
  sel: Selection | null = typeof window === "undefined" ? null : window.getSelection(),
): AssistantSelection | null {
  if (!sel || sel.rangeCount === 0 || sel.isCollapsed) return null;
  const range = sel.getRangeAt(0);
  if (!root.contains(range.commonAncestorContainer)) return null;
  const startRow = rowOf(range.startContainer);
  const endRow = rowOf(range.endContainer);
  if (!startRow || startRow !== endRow) return null;
  // A selection that started in the row's own controls (copy / read buttons)
  // is not prose either.
  if (
    (range.startContainer as Element | Node).parentElement?.closest("button, input, textarea")
  ) {
    return null;
  }
  const quote = sel.toString().replace(/\s+\n/g, "\n").trim();
  if (quote.length < MIN_QUOTE_CHARS) return null;
  const idx = Number(startRow.getAttribute("data-msg-idx"));
  if (!Number.isFinite(idx)) return null;
  // jsdom's Range has no layout; a zero box keeps the rule testable there.
  const rect: DOMRect =
    typeof range.getBoundingClientRect === "function"
      ? range.getBoundingClientRect()
      : ({ left: 0, top: 0, width: 0, height: 0, right: 0, bottom: 0, x: 0, y: 0 } as DOMRect);
  return {
    quote: quote.length > MAX_QUOTE_CHARS ? quote.slice(0, MAX_QUOTE_CHARS) : quote,
    msgIdx: idx,
    msgUuid: startRow.getAttribute("data-msg-uuid"),
    rect,
  };
}

/**
 * Select `quote` inside `el` natively, so the passage a card was asked about
 * lights up the way the reader originally marked it. Matches the first
 * occurrence across text nodes (markdown splits a sentence over many spans);
 * whitespace runs are folded on both sides so a quote copied from rendered
 * prose still finds its source. Returns whether a match was selected.
 */
export function selectQuoteIn(el: HTMLElement, quote: string): boolean {
  const target = quote.replace(/\s+/g, " ").trim();
  if (!target || typeof document === "undefined") return false;
  const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
  let hay = "";
  // Fold whitespace while keeping a map from folded offsets back to nodes.
  const map: { node: Text; offset: number }[] = [];
  let lastWasSpace = true;
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    const text = n as Text;
    const data = text.data;
    for (let i = 0; i < data.length; i++) {
      const ch = data[i];
      if (/\s/.test(ch)) {
        if (lastWasSpace) continue;
        lastWasSpace = true;
        hay += " ";
      } else {
        lastWasSpace = false;
        hay += ch;
      }
      map.push({ node: text, offset: i });
    }
  }
  const at = hay.indexOf(target);
  if (at < 0) return false;
  const first = map[at];
  const last = map[at + target.length - 1];
  if (!first || !last) return false;
  const range = document.createRange();
  range.setStart(first.node, first.offset);
  range.setEnd(last.node, last.offset + 1);
  const sel = window.getSelection();
  if (!sel) return false;
  sel.removeAllRanges();
  sel.addRange(range);
  return true;
}

/** A chip-length preview of the quote: first line, ellipsised. */
export function quoteSnippet(quote: string, max = 40): string {
  const line = quote.trim().split("\n")[0]?.trim() ?? "";
  return line.length > max ? `${line.slice(0, max - 1)}…` : line;
}

/**
 * Share of the prompt that came from cache, 0–1, or `null` before any usage
 * landed. The number the card exists to show: it is what makes a fork of a
 * warm session cost cents and a cold one dollars.
 */
export function cacheHitRatio(rec: Pick<ExplainRecord, "inputTokens" | "cacheReadTokens" | "cacheCreationTokens">): number | null {
  const prompt = rec.inputTokens + rec.cacheReadTokens + rec.cacheCreationTokens;
  if (prompt <= 0) return null;
  return rec.cacheReadTokens / prompt;
}

/** `$0.07` / `$1.6` / `<$0.01` — sized for a 10px mono line. */
export function costLabel(usd: number | null | undefined): string {
  if (usd == null || !Number.isFinite(usd)) return "";
  if (usd === 0) return "$0";
  if (usd < 0.01) return "<$0.01";
  return `$${usd < 1 ? usd.toFixed(2) : usd.toFixed(1)}`;
}
