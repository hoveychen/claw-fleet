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
 * Order is deliberate: the **nav sidebar** goes first — collapsed it is still a
 * usable icon rail, so the cost is nearly nothing — and only if the prose is
 * *still* short does the **secondary sidebar** (the session list) follow, which
 * costs more because it is the thing you switch sessions with. Nothing is ever
 * auto-collapsed twice, and only what this hook took is restored (see the
 * store's `autoCollapsed`), so a panel the reader had already folded by hand
 * does not spring open when a doc card closes.
 *
 * One step per pass on purpose: collapsing the sidebar widens the pane, the
 * ResizeObserver in `useDocCardWidth` reports the new width, and this runs
 * again against a real measurement rather than a prediction of one.
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
  const sidebarCollapsed = useUIStore((s) => s.sidebarCollapsed);
  const secondaryCollapsed = useUIStore((s) => !!s.secondarySidebarCollapsed[view]);

  useEffect(() => {
    if (!enabled || !active) return;
    if (proseW <= 0 || proseW >= PROSE_MIN_PX) return;
    if (!sidebarCollapsed) {
      autoCollapse("sidebar");
      return;
    }
    // Only pages built on PageShell have a secondary sidebar to fold; on the
    // others (会话, whose list column is the page itself) step 1 is all there
    // is, and setting the flag would record a collapse nothing performs.
    if (RAILS[view] && !secondaryCollapsed) autoCollapse(view);
  }, [enabled, active, proseW, sidebarCollapsed, secondaryCollapsed, view, autoCollapse]);

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
