/** Where a pending decision card can actually be drawn in this host.
 *
 * There are two surfaces: the in-window `DecisionPanel` (`inline`) and the
 * standalone decision-float window (`float`). The desktop picks `float` when
 * the user cannot see the in-window panel — the main window is minimized, they
 * turned on "always use the standalone window", or they are in Lite mode
 * (which renders no in-window card by design).
 *
 * The browser build has no second window to pop. `show_decision_float` /
 * `hide_decision_float` answer `null` there (`webTransport.ts`), so choosing
 * `float` in a tab means the card sits in the store with nothing rendering it
 * and nothing logged — the agent blocks until it times out while the page
 * looks idle. Observed on `fleet-cloud.muveeai.com`: a `fleet__ask` card was
 * live for ten minutes and never appeared, and the only way out was
 * interrupting the turn.
 *
 * Hence the one rule this module exists to state: a host that cannot float
 * always renders inline.
 */
export type DecisionSurface = "inline" | "float";

export interface DecisionSurfaceInput {
  /** This page is the browser build (`fleet webui`), not the desktop webview.
   *  Read from `hostEnv.isWebBuild()` — a boot-time flag, not a live probe. */
  webBuild: boolean;
  /** Settings: "always use the standalone decision window". */
  floatingPreferred: boolean;
  /** Lite mode renders no in-window decision card on the desktop. */
  liteMode: boolean;
  /** The desktop main window is minimized, so its in-window panel is unseen. */
  mainMinimized: boolean;
}

export function decisionSurface(i: DecisionSurfaceInput): DecisionSurface {
  // Checked first, and unconditionally: a tab cannot float, so none of the
  // three preferences below can mean anything there. Honouring them would
  // route the card to a window that does not exist.
  if (i.webBuild) return "inline";
  return i.mainMinimized || i.floatingPreferred || i.liteMode ? "float" : "inline";
}
