/** Which surfaces may draw a pending decision card in this host.
 *
 * There are two, and they are not exclusive: the in-window `DecisionPanel`
 * (`inline`) and the standalone decision-float window (`float`). The desktop
 * pops the float when the user cannot see the in-window panel — the main
 * window is minimized, they turned on "always use the standalone window", or
 * they are in Lite mode (which renders no in-window card by design). A
 * minimized main window keeps its inline panel mounted underneath, so both
 * flags are true at once there; that is why this returns a pair rather than a
 * single choice. `inline` is what the App's Lite branch and its normal branch
 * both gate on, so Lite has to clear it explicitly — the Lite branch renders a
 * different tree, not a variant of the normal one.
 *
 * The browser build has no second window to pop. `show_decision_float` /
 * `hide_decision_float` answer `null` there (`webTransport.ts`), so preferring
 * the float in a tab leaves the card sitting in the store with nothing
 * rendering it and nothing logged — the agent blocks until it times out while
 * the page looks idle. Observed on `fleet-cloud.muveeai.com`: a `fleet__ask`
 * card was live for ten minutes, never appeared, and the only way out was
 * interrupting the turn.
 *
 * Hence the one rule this module exists to state: a host that cannot float
 * always renders inline, whatever the three preferences say.
 */
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

export interface DecisionSurfaces {
  /** Mount the in-window `DecisionPanel`. */
  inline: boolean;
  /** Pop the standalone decision-float window. */
  float: boolean;
}

export function decisionSurfaces(i: DecisionSurfaceInput): DecisionSurfaces {
  // Checked first, and unconditionally: a tab cannot float, so none of the
  // three preferences below can mean anything there. Honouring them would
  // route the card to a window that does not exist.
  if (i.webBuild) return { inline: true, float: false };
  return {
    inline: !i.floatingPreferred && !i.liteMode,
    float: i.mainMinimized || i.floatingPreferred || i.liteMode,
  };
}
