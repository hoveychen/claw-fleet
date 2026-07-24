import { useCallback, useEffect, useRef, useState } from "react";
import { findMatches, type FindMatch } from "./domSearch";
import {
  frameClear,
  frameGoto,
  frameSearch,
  listSearchableFrames,
  parseFindResult,
} from "./iframeBridge";

/**
 * Drives the in-app Cmd+F find bar across the main document AND sandboxed
 * preview iframes.
 *
 * Main-document highlighting uses the CSS Custom Highlight API (`CSS.highlights`
 * + `::highlight()`), so matches are painted without mutating the DOM — no
 * `<mark>` wrappers to confuse React's reconciler. Iframe previews are opaque to
 * the parent, so they search themselves via an injected handler
 * (`mcp_ipc::FIND_SCRIPT`) and report their counts back (see iframeBridge).
 *
 * Navigation is unified: every main-document match and every iframe match folds
 * into one list ordered by vertical position, so ↑/↓ walks the page top-to-bottom
 * regardless of which side of the sandbox a match lives on.
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

function clearMainHighlights() {
  const reg = highlightRegistry();
  if (!reg) return;
  reg.delete(HL_ALL);
  reg.delete(HL_CURRENT);
}

/**
 * Elements whose subtree must never be searched. The search is already scoped to
 * the active page's content root (see {@link searchRoots}), so this additionally
 * drops in-content chrome — the sidebar/nav, buttons and menus the boss asked to
 * keep out of results — plus never-visible markup.
 */
function shouldSkip(el: Element): boolean {
  const tag = el.tagName;
  if (tag === "SCRIPT" || tag === "STYLE" || tag === "NOSCRIPT") return true;
  if (tag === "ASIDE" || tag === "NAV" || tag === "BUTTON") return true;
  // The find bar's own chrome (input, count, buttons) must not match itself.
  if (el.hasAttribute("data-find-bar")) return true;
  if (el.getAttribute("aria-hidden") === "true") return true;
  if ((el as HTMLElement).hidden) return true;
  return false;
}

/**
 * The content roots to search: the current page's content container(s), tagged
 * `data-find-content`. Only visible ones count, so a mounted-but-offscreen view
 * doesn't leak matches. Falls back to `<body>` if nothing is tagged (e.g. lite
 * mode), which still beats searching nothing.
 */
function searchRoots(): Element[] {
  const tagged = Array.from(document.querySelectorAll("[data-find-content]")).filter(
    (el) => el.getClientRects().length > 0,
  );
  return tagged.length ? tagged : [document.body];
}

function rangeFor(m: FindMatch): Range {
  const r = document.createRange();
  r.setStart(m.node, m.start);
  r.setEnd(m.node, m.end);
  return r;
}

/** A frame that has reported at least one match. */
interface FrameEntry {
  frame: HTMLIFrameElement;
  count: number;
}

/** One entry in the unified, vertically-ordered navigation list. */
type OrderEntry =
  | { kind: "main"; mainIdx: number }
  | { kind: "frame"; frameIdx: number; localIdx: number };

export interface FindController {
  open: boolean;
  query: string;
  /** Total matches across the main document and all preview iframes. */
  total: number;
  /** 0-based index of the active match in the unified order, or -1 when none. */
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

  const mainRef = useRef<FindMatch[]>([]);
  const framesRef = useRef<FrameEntry[]>([]);
  const orderRef = useRef<OrderEntry[]>([]);
  const activeRef = useRef(-1);

  const paintMainAll = useCallback(() => {
    const reg = highlightRegistry();
    if (!reg) return;
    const matches = mainRef.current;
    if (matches.length) reg.set(HL_ALL, new Highlight(...matches.map(rangeFor)));
    else reg.delete(HL_ALL);
  }, []);

  // Rebuild the unified order from current main matches + frame counts, sorting
  // by each match's vertical position so ↑/↓ moves down the page. An iframe's
  // matches all sort at the iframe element's top (we can't see inside), forming
  // a contiguous block ordered by the iframe's own local index.
  const rebuildOrder = useCallback(() => {
    const anchored: { top: number; sub: number; entry: OrderEntry }[] = [];
    mainRef.current.forEach((m, i) => {
      let top = 0;
      try {
        top = rangeFor(m).getBoundingClientRect().top;
      } catch {
        /* detached node — sort it to the top */
      }
      anchored.push({ top, sub: 0, entry: { kind: "main", mainIdx: i } });
    });
    framesRef.current.forEach((f, fi) => {
      if (f.count <= 0) return;
      const top = f.frame.getBoundingClientRect().top;
      for (let k = 0; k < f.count; k++) {
        anchored.push({ top, sub: k, entry: { kind: "frame", frameIdx: fi, localIdx: k } });
      }
    });
    anchored.sort((a, b) => a.top - b.top || a.sub - b.sub);
    orderRef.current = anchored.map((a) => a.entry);
    setTotal(orderRef.current.length);
  }, []);

  // Move the active mark to global index `idx`: clear every current mark (main +
  // all frames), then set the one the target lives in and scroll it into view.
  const applyActive = useCallback((idx: number) => {
    const reg = highlightRegistry();
    reg?.delete(HL_CURRENT);
    framesRef.current.forEach((f) => frameGoto(f.frame, -1));

    activeRef.current = idx;
    setActiveIndex(idx);

    const entry = orderRef.current[idx];
    if (!entry) return;
    if (entry.kind === "main") {
      const m = mainRef.current[entry.mainIdx];
      if (m && reg) reg.set(HL_CURRENT, new Highlight(rangeFor(m)));
      m?.node.parentElement?.scrollIntoView({ block: "center" });
    } else {
      const f = framesRef.current[entry.frameIdx];
      if (f) {
        f.frame.scrollIntoView({ block: "center" });
        frameGoto(f.frame, entry.localIdx);
      }
    }
  }, []);

  const runSearch = useCallback(
    (q: string) => {
      const roots = searchRoots();
      // querySelectorAll returns roots in document order and findMatches is in
      // order within each, so the concatenation is in document order (the final
      // navigation order is re-derived by vertical position in rebuildOrder).
      mainRef.current = q ? roots.flatMap((root) => findMatches(root, q, shouldSkip)) : [];
      paintMainAll();

      // Reset frame bookkeeping and (re)issue the search to every candidate iframe
      // that lives inside a content root; the ones carrying the handler reply with
      // a count, the rest stay at 0.
      const frames = listSearchableFrames()
        .filter((f) => roots.some((r) => r.contains(f)))
        .map((frame) => ({ frame, count: 0 }));
      framesRef.current = frames;
      if (q) frames.forEach((f) => frameSearch(f.frame, q));
      else frames.forEach((f) => frameClear(f.frame));

      rebuildOrder();
      applyActive(orderRef.current.length ? 0 : -1);
    },
    [paintMainAll, rebuildOrder, applyActive],
  );

  const setQuery = useCallback(
    (q: string) => {
      setQueryState(q);
      runSearch(q);
    },
    [runSearch],
  );

  const step = useCallback(
    (delta: number) => {
      const n = orderRef.current.length;
      if (!n) return;
      applyActive((activeRef.current + delta + n) % n);
    },
    [applyActive],
  );

  const next = useCallback(() => step(1), [step]);
  const prev = useCallback(() => step(-1), [step]);

  const openBar = useCallback(() => setOpen(true), []);

  const close = useCallback(() => {
    setOpen(false);
    setQueryState("");
    mainRef.current = [];
    framesRef.current.forEach((f) => frameClear(f.frame));
    framesRef.current = [];
    orderRef.current = [];
    activeRef.current = -1;
    setTotal(0);
    setActiveIndex(-1);
    clearMainHighlights();
  }, []);

  // Collect match counts as preview iframes reply, then re-fold the order. An
  // iframe on an opaque origin reports `e.origin === "null"`, so we match on
  // `e.source` (its window) rather than origin.
  useEffect(() => {
    const onMessage = (e: MessageEvent) => {
      const result = parseFindResult(e.data);
      if (!result) return;
      const entry = framesRef.current.find((f) => f.frame.contentWindow === e.source);
      if (!entry) return;
      entry.count = result.count;
      rebuildOrder();
      // First real match after a search that had none? Land the active mark.
      if (activeRef.current < 0 && orderRef.current.length) applyActive(0);
    };
    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  }, [rebuildOrder, applyActive]);

  // Cmd/Ctrl+F opens the bar; Escape (when open) closes it. Capture phase so we
  // beat component-level handlers, and preventDefault stops the webview's own
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
  useEffect(() => () => clearMainHighlights(), []);

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
