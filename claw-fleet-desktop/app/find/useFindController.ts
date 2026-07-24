import { useCallback, useEffect, useRef, useState } from "react";
import { findMatches, type FindMatch } from "./domSearch";

/**
 * Drives the in-app Cmd+F find bar for the main document.
 *
 * Highlighting uses the CSS Custom Highlight API (`CSS.highlights` +
 * `::highlight()`), so matches are painted without mutating the DOM — no
 * `<mark>` wrappers to confuse React's reconciler. Two named highlights are
 * registered: `find-match` for every hit and `find-current` for the active one.
 *
 * P1 scope: the main document only. The iframe bridge (P3) layers sandboxed
 * previews on top via the same open/query state.
 */

const HL_ALL = "find-match";
const HL_CURRENT = "find-current";

/** Whether the CSS Custom Highlight API is usable in this webview. */
export const highlightSupported =
  typeof CSS !== "undefined" &&
  !!CSS.highlights &&
  typeof Highlight !== "undefined" &&
  typeof Range !== "undefined";

function highlightRegistry(): Map<string, Highlight> | null {
  if (!highlightSupported) return null;
  return CSS.highlights as unknown as Map<string, Highlight>;
}

function clearHighlights() {
  const reg = highlightRegistry();
  if (!reg) return;
  reg.delete(HL_ALL);
  reg.delete(HL_CURRENT);
}

/** Elements whose subtree must never be searched. */
function shouldSkip(el: Element): boolean {
  const tag = el.tagName;
  if (tag === "SCRIPT" || tag === "STYLE" || tag === "NOSCRIPT") return true;
  // The find bar's own chrome (input, count, buttons) must not match itself.
  if (el.hasAttribute("data-find-bar")) return true;
  if (el.getAttribute("aria-hidden") === "true") return true;
  if ((el as HTMLElement).hidden) return true;
  return false;
}

function rangeFor(m: FindMatch): Range {
  const r = document.createRange();
  r.setStart(m.node, m.start);
  r.setEnd(m.node, m.end);
  return r;
}

export interface FindController {
  open: boolean;
  query: string;
  /** Number of matches in the main document. */
  total: number;
  /** 0-based index of the active match, or -1 when there are none. */
  activeIndex: number;
  supported: boolean;
  setQuery: (q: string) => void;
  next: () => void;
  prev: () => void;
  openBar: () => void;
  close: () => void;
}

export function useFindController(): FindController {
  const [open, setOpen] = useState(false);
  const [query, setQueryState] = useState("");
  const [total, setTotal] = useState(0);
  const [activeIndex, setActiveIndex] = useState(-1);

  const matchesRef = useRef<FindMatch[]>([]);

  const paintCurrent = useCallback((index: number) => {
    const reg = highlightRegistry();
    if (!reg) return;
    const matches = matchesRef.current;
    if (index < 0 || index >= matches.length) {
      reg.delete(HL_CURRENT);
      return;
    }
    const current = rangeFor(matches[index]);
    reg.set(HL_CURRENT, new Highlight(current));
    matches[index].node.parentElement?.scrollIntoView({ block: "center" });
  }, []);

  const recompute = useCallback(
    (q: string) => {
      const reg = highlightRegistry();
      const matches = q ? findMatches(document.body, q, shouldSkip) : [];
      matchesRef.current = matches;
      setTotal(matches.length);
      if (reg) {
        if (matches.length) {
          reg.set(HL_ALL, new Highlight(...matches.map(rangeFor)));
        } else {
          reg.delete(HL_ALL);
        }
      }
      const nextActive = matches.length ? 0 : -1;
      setActiveIndex(nextActive);
      paintCurrent(nextActive);
    },
    [paintCurrent],
  );

  const setQuery = useCallback(
    (q: string) => {
      setQueryState(q);
      recompute(q);
    },
    [recompute],
  );

  const step = useCallback(
    (delta: number) => {
      const n = matchesRef.current.length;
      if (!n) return;
      setActiveIndex((cur) => {
        const next = (cur + delta + n) % n;
        paintCurrent(next);
        return next;
      });
    },
    [paintCurrent],
  );

  const next = useCallback(() => step(1), [step]);
  const prev = useCallback(() => step(-1), [step]);

  const openBar = useCallback(() => {
    setOpen(true);
  }, []);

  const close = useCallback(() => {
    setOpen(false);
    setQueryState("");
    matchesRef.current = [];
    setTotal(0);
    setActiveIndex(-1);
    clearHighlights();
  }, []);

  // Cmd/Ctrl+F opens the bar; Escape (when open) closes it. Capture phase so we
  // beat any component-level handlers, and preventDefault stops the webview's own
  // (non-functional) find affordance.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && !e.altKey && (e.key === "f" || e.key === "F")) {
        e.preventDefault();
        setOpen(true);
      } else if (e.key === "Escape" && open) {
        close();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [open, close]);

  // Clear paint if the component tree tears down while matches are live.
  useEffect(() => () => clearHighlights(), []);

  return {
    open,
    query,
    total,
    activeIndex,
    supported: highlightSupported,
    setQuery,
    next,
    prev,
    openBar,
    close,
  };
}
