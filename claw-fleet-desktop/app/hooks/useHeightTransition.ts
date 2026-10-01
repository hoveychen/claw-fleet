import { useLayoutEffect, useRef, type RefObject } from "react";

/** True when the OS asks for reduced motion. Read per call, not cached, so a
 *  setting flipped while the app is open takes effect on the next swap. */
export function prefersReducedMotion(): boolean {
  return typeof window !== "undefined"
    && typeof window.matchMedia === "function"
    && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/**
 * Animates an element's height when `swapKey` changes and the element's
 * content is replaced by something taller or shorter — e.g. the sidebar nav
 * swapping its top-level list for the "More" sub-page. Without it the swap
 * lands in one frame and everything below the element jumps.
 *
 * Height is otherwise left to layout (`style.height` is cleared once the
 * transition ends), so resizing, collapsing the rail or a short window that
 * makes the element scroll all behave exactly as before. The first render and
 * reduced-motion users get no animation.
 */
export function useHeightTransition(
  ref: RefObject<HTMLElement | null>,
  swapKey: unknown,
  durationMs: number,
): void {
  // Height as of the last commit, i.e. before this swap's new content.
  const lastHeight = useRef<number | null>(null);
  const mounted = useRef(false);

  // Keep `lastHeight` current through changes that are not swaps (rail
  // collapse, window resize), so the next swap starts from the real height.
  // Readings taken mid-transition are skipped; the one after `clear()` lands.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => {
      if (el.style.height === "") lastHeight.current = el.getBoundingClientRect().height;
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const from = lastHeight.current;
    const to = el.getBoundingClientRect().height;
    lastHeight.current = to;
    if (!mounted.current) {
      mounted.current = true;
      return;
    }
    if (from === null || Math.abs(from - to) < 1 || prefersReducedMotion()) return;

    el.style.height = `${from}px`;
    el.style.transition = `height ${durationMs}ms var(--ease-out)`;
    // Commit the start height before moving to the end one, or the browser
    // folds both writes into a single style change and nothing animates.
    void el.offsetHeight;
    el.style.height = `${to}px`;

    const clear = () => {
      el.style.height = "";
      el.style.transition = "";
    };
    // transitionend is not guaranteed (a swap interrupted mid-flight, a hidden
    // window), so a timer backs it up.
    const timer = window.setTimeout(clear, durationMs + 50);
    const onEnd = (e: TransitionEvent) => {
      if (e.target === el && e.propertyName === "height") clear();
    };
    el.addEventListener("transitionend", onEnd);
    return () => {
      window.clearTimeout(timer);
      el.removeEventListener("transitionend", onEnd);
      clear();
    };
  }, [swapKey]);
}
