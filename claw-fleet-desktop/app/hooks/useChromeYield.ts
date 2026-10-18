import { useEffect } from "react";
import { useUIStore, type ViewMode } from "../store";
import { RAILS } from "../components/pageShellConfig";

/** How much room the conversation needs before the layout starts giving up
 *  chrome for it. Below this a transcript is a column of two-word lines. */
export const PROSE_MIN_PX = 420;

/**
 * Fold the app's chrome away while a wide reader is open in a pane too narrow
 * to hold both, and put it back when the reader closes.
 *
 * The nav is a fixed icon rail with nothing left to fold, so the only chrome
 * that yields is the page's **secondary sidebar** (the task list). Nothing is
 * ever auto-collapsed twice, and only what this hook took is restored (see the
 * store's `autoCollapsed`), so a panel the reader had already folded by hand
 * does not spring open when a doc card closes.
 */
export function useChromeYield({
  active,
  view,
  proseW,
  enabled,
}: {
  /** A wide reader is open. */
  active: boolean;
  /** Whose secondary sidebar to fold — the page this pane is on. */
  view: ViewMode;
  /** What the conversation is left with right now, in px. */
  proseW: number;
  /** Off for embedded hosts: an inline SessionDetail is a quadrant of someone
   *  else's layout and has no business folding the window's chrome. */
  enabled: boolean;
}) {
  const autoCollapse = useUIStore((s) => s.autoCollapse);
  const autoRestore = useUIStore((s) => s.autoRestore);
  const secondaryCollapsed = useUIStore((s) => !!s.secondarySidebarCollapsed[view]);

  useEffect(() => {
    if (!enabled || !active) return;
    if (proseW <= 0 || proseW >= PROSE_MIN_PX) return;
    // Only pages built on PageShell have a secondary sidebar to fold; setting
    // the flag elsewhere would record a collapse nothing performs.
    if (RAILS[view] && !secondaryCollapsed) autoCollapse(view);
  }, [enabled, active, proseW, secondaryCollapsed, view, autoCollapse]);

  // Closing the reader — or leaving the pane entirely — hands the chrome back.
  useEffect(() => {
    if (!enabled || active) return;
    autoRestore();
  }, [enabled, active, autoRestore]);
  useEffect(() => {
    if (!enabled) return;
    return () => autoRestore();
  }, [enabled, autoRestore]);
}
