/**
 * Side questions about a session's transcript ("selection explain") — the
 * client-side pieces both the desktop and the phone need verbatim.
 *
 * The host answers in a fork on its own thread and rewrites the record file
 * as text streams in (`claw-fleet-core/src/session_explain.rs`), so
 * "streaming" on a client is nothing more than re-reading the record until
 * its status leaves `running`. The poll loop, the labels a card shows, and
 * the DOM rules for what counts as a quotable selection are the same on both
 * surfaces; only the transport differs (Tauri `invoke` vs the relay), which
 * is why everything here takes its fetch injected and its record structurally.
 *
 * The record shape is the generated `ExplainRecord` in each package's
 * `generated/types.ts`; this module cannot import either, so it names the
 * fields it reads.
 */

/** The fields of a record the poll loop and the labels read. */
export interface ExplainRecordLike {
  status: string;
  updatedMs: number;
  text: string;
}

/** How often a running record is re-read. The host flushes every ~120 ms. */
export const EXPLAIN_POLL_MS = 300;

/**
 * Re-read a record until it settles, reporting every observed change.
 *
 * `fetch` is injected so the loop is testable and reusable by any transport.
 * A fetch failure is treated as "not yet readable" and retried: the record is
 * written atomically, but the first read can race the worker's first flush.
 * `signal` aborts the loop; the promise then resolves with the last record
 * seen (or `null`).
 */
export async function pollExplanation<T extends ExplainRecordLike>(
  fetch: () => Promise<T>,
  onUpdate: (rec: T) => void,
  opts: { intervalMs?: number; signal?: AbortSignal; sleep?: (ms: number) => Promise<void> } = {},
): Promise<T | null> {
  const interval = opts.intervalMs ?? EXPLAIN_POLL_MS;
  const sleep = opts.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));
  let last: T | null = null;
  let lastKey = "";
  while (!opts.signal?.aborted) {
    let rec: T | null = null;
    try {
      rec = await fetch();
    } catch {
      rec = null;
    }
    if (rec) {
      const key = `${rec.status}:${rec.updatedMs}:${rec.text.length}`;
      if (key !== lastKey) {
        lastKey = key;
        last = rec;
        onUpdate(rec);
      }
      if (rec.status !== "running") return rec;
    }
    await sleep(interval);
  }
  return last;
}

// ── Labels ───────────────────────────────────────────────────────────────────

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
export function cacheHitRatio(rec: {
  inputTokens: number;
  cacheReadTokens: number;
  cacheCreationTokens: number;
}): number | null {
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

// ── Selection (DOM) ──────────────────────────────────────────────────────────
//
// The transcript renders every message row with a `data-msg-idx`, and
// assistant rows additionally carry `data-role="assistant"` and, when the
// record had one, `data-msg-uuid` (desktop MessageList, mobile
// SessionDetailView). A selection qualifies when it is non-empty and *both*
// ends sit inside one assistant row: a drag that starts in the user's bubble
// or spans two turns is not a passage the agent wrote, and the fork prompt
// quotes it as one. Pure DOM, no React, so the rules can be tested in jsdom
// without mounting the transcript.

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
 * Fired (bubbling) on an explain mark right after `selectExplainMark` set the
 * selection to it, so the ask bars can read the selection at once instead of
 * waiting for a `mouseup` that a keyboard activation never produces.
 */
export const EXPLAIN_MARK_SELECT_EVENT = "fleet:explain-mark-select";

/**
 * Make one of the agent's `[?text]` marks the current selection, exactly as
 * if the reader had dragged over it, then announce it. The ask bar that owns
 * the surrounding transcript (or card) reads the selection through
 * `readAssistantSelection` and shows its presets — clicking a mark is a
 * shortcut to selecting, not to asking. Returns whether a selection was made.
 */
export function selectExplainMark(el: HTMLElement): boolean {
  if (typeof window === "undefined" || typeof document === "undefined") return false;
  const sel = window.getSelection();
  if (!sel) return false;
  const range = document.createRange();
  range.selectNodeContents(el);
  sel.removeAllRanges();
  sel.addRange(range);
  el.dispatchEvent(new CustomEvent(EXPLAIN_MARK_SELECT_EVENT, { bubbles: true }));
  return true;
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

/**
 * Scroll the transcript row a side question was asked about into view and
 * flash it. The uuid is the durable key; the index is the fallback for a
 * record (or a row) without one. Returns the row, so the caller can
 * re-select the quote in it, or `null` when the row is not rendered (older
 * than the loaded tail window, for instance).
 */
export function locateExplainRow(
  root: HTMLElement,
  anchor: { msgUuid?: string | null; msgIdx?: number | null } | null | undefined,
  flashClass: string,
  flashMs = 1700,
): HTMLElement | null {
  let row: HTMLElement | null = null;
  const uuid = anchor?.msgUuid;
  if (uuid && /^[\w-]+$/.test(uuid)) {
    row = root.querySelector<HTMLElement>(`[data-msg-uuid="${uuid}"]`);
  }
  if (!row && anchor?.msgIdx != null) {
    row = root.querySelector<HTMLElement>(`[data-msg-idx="${anchor.msgIdx}"]`);
  }
  if (!row) return null;
  row.scrollIntoView({ behavior: "smooth", block: "center" });
  row.classList.remove(flashClass);
  // Restart the animation even when the same row is located twice in a row.
  void row.offsetWidth;
  row.classList.add(flashClass);
  const target = row;
  window.setTimeout(() => target.classList.remove(flashClass), flashMs);
  return row;
}
