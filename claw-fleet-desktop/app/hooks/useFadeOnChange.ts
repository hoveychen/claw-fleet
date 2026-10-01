import { useLayoutEffect, useRef, type RefObject } from "react";
import { prefersReducedMotion } from "./useHeightTransition";

/**
 * Fades an element in whenever `key` changes after the first render — the
 * detail column picking up a different session / doc / skill. Uses the Web
 * Animations API instead of a keyed remount, because the detail components are
 * reused across selections on purpose (SessionDetail keeps its tail poller and
 * scroll-follow state; remounting would refetch the transcript on every click).
 *
 * Opacity only: a transform would make the element the containing block for
 * any position:fixed dialog rendered inside it. A null/undefined key (nothing
 * selected) never animates, and neither does the step from nothing to the
 * first selection that a page restores on mount.
 */
export function useFadeOnChange(
  ref: RefObject<HTMLElement | null>,
  key: string | number | null | undefined,
  durationMs = 160,
): void {
  const prev = useRef(key);
  // Layout effect: start before the new content is painted, or it shows at
  // full opacity for one frame and then dips.
  useLayoutEffect(() => {
    const was = prev.current;
    prev.current = key;
    if (was === key || was == null || key == null) return;
    const el = ref.current;
    if (!el || typeof el.animate !== "function" || prefersReducedMotion()) return;
    const anim = el.animate([{ opacity: 0.35 }, { opacity: 1 }], {
      duration: durationMs,
      easing: "cubic-bezier(0.2, 0, 0, 1)",
    });
    return () => anim.cancel();
  }, [key]);
}
