/**
 * Key → split-action resolution for the 启动台 detail column.
 *
 * Split apart from the hook that listens on `window` so the fiddly part — the
 * ⌘K prefix chord, and which strokes get swallowed — is testable without a DOM.
 *
 * Bindings follow VS Code:
 *
 *   ⌘\        split right
 *   ⌘K ⌘\     split down
 *   ⌘1..⌘9    focus the N-th group
 *
 * `⌘W` (close editor) is deliberately **not** bound: on macOS it is also
 * "Close Window" on Tauri's default app menu, and a shortcut that might shut the
 * window instead of a tab is not a trade worth making for a keystroke the ✕ and
 * middle-click already cover.
 */

/** What a resolved stroke asks the column to do. */
export type SplitAction =
  | { kind: "splitRight" }
  | { kind: "splitDown" }
  | { kind: "focusGroup"; pos: number };

/** Chord progress. `null` = idle; `"k"` = ⌘K seen, second stroke pending. */
export type ChordState = null | "k";

export interface KeyResult {
  /** The action to run, if this stroke completed one. */
  action: SplitAction | null;
  /** Chord state to carry into the next stroke. */
  chord: ChordState;
  /** Did we claim the stroke? Claimed strokes get `preventDefault`. */
  handled: boolean;
}

/** The subset of KeyboardEvent this depends on, so tests need no DOM. */
export interface KeyLike {
  key: string;
  metaKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}

const IGNORE: KeyResult = { action: null, chord: null, handled: false };

/**
 * Keys that are a modifier *by themselves*. Chromium fires a keydown for these
 * before the letter of a combination — and that event already reports
 * `metaKey: true`, so it looks exactly like a modified stroke. Treating it as
 * one cancels a half-typed chord, which is how ⌘K ⌘\ silently degraded into
 * ⌘\ (split down became split right) whenever the user let go of ⌘ in between.
 */
const MODIFIER_KEYS = new Set([
  "Meta",
  "Control",
  "Shift",
  "Alt",
  "AltGraph",
  "CapsLock",
]);

/**
 * Resolve one keystroke against the current chord state.
 *
 * A stroke without the platform modifier (or with Alt/Shift, which belong to
 * other bindings) resets a half-typed chord rather than leaving it armed — a
 * chord that survives arbitrary typing would fire a split minutes later on an
 * unrelated `⌘\`.
 */
export function resolveSplitKey(e: KeyLike, chord: ChordState): KeyResult {
  // Not a stroke at all — pass the chord through untouched.
  if (MODIFIER_KEYS.has(e.key)) return { action: null, chord, handled: false };

  const mod = e.metaKey || e.ctrlKey;
  if (!mod || e.altKey || e.shiftKey) {
    // Modifier-less typing doesn't *consume* the chord (holding ⌘ down between
    // the two strokes emits no bare keys), but any other modified stroke does.
    return mod ? IGNORE : { action: null, chord, handled: false };
  }

  if (chord === "k") {
    // Second half of ⌘K ⌘\ — anything else cancels the chord without acting.
    if (e.key === "\\") return { action: { kind: "splitDown" }, chord: null, handled: true };
    return IGNORE;
  }

  if (e.key === "\\") return { action: { kind: "splitRight" }, chord: null, handled: true };
  // Swallow the prefix itself, or the browser/webview may act on plain ⌘K.
  if (e.key === "k" || e.key === "K") return { action: null, chord: "k", handled: true };
  if (e.key >= "1" && e.key <= "9" && e.key.length === 1) {
    return { action: { kind: "focusGroup", pos: Number(e.key) }, chord: null, handled: true };
  }
  return IGNORE;
}

/** How long a half-typed ⌘K stays armed. Matches VS Code's chord timeout
 *  closely enough that muscle memory transfers, and short enough that a
 *  forgotten prefix can't hijack a later stroke. */
export const CHORD_TIMEOUT_MS = 1500;
