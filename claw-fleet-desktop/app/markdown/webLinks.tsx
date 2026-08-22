import { createContext, useContext, type ReactNode } from "react";

/**
 * Where an external `http(s)` link in markdown opens.
 *
 * Everywhere in the app it goes straight to the system browser — which is right
 * for a decision card or a chat bubble, and wrong for the 任务 page's detail
 * column, where the whole point is to keep the linked page beside the prose that
 * cited it. So a surface with a tab strip provides an opener here, and the link
 * renderers (safeLinks, wikiLinks) consult it before falling back.
 *
 * Ambient rather than a prop for the same reason as `wikiLinksContext`: the link
 * renderers sit at the bottom of a deep tree of block renderers, none of which
 * should have to know about this.
 */
const WebLinkCtx = createContext<((url: string) => void) | null>(null);

export function WebLinkProvider({
  value,
  children,
}: {
  value: ((url: string) => void) | null;
  children: ReactNode;
}) {
  return <WebLinkCtx.Provider value={value}>{children}</WebLinkCtx.Provider>;
}

/** The ambient in-app opener, or `null` where links belong in the browser. */
export function useWebLinkTarget(): ((url: string) => void) | null {
  return useContext(WebLinkCtx);
}

/**
 * Did this click ask for the *system browser* rather than an in-app tab?
 *
 * ⌘/Ctrl-click and middle-click are the gestures every browser already spends on
 * "open this somewhere other than here", so they stay the escape hatch when an
 * in-app tab is the default. Alt is deliberately not one of them: browsers spend
 * it on download.
 */
export function wantsSystemBrowser(e: {
  button: number;
  metaKey: boolean;
  ctrlKey: boolean;
}): boolean {
  return e.button === 1 || e.metaKey || e.ctrlKey;
}
