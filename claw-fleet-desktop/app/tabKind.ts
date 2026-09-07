/**
 * What a detail-column tab *holds*.
 *
 * A tab is a session — either the pane you work in or a second view of the same
 * one — plus the synthetic `new:draft`. Material the conversation *cites* (a
 * repo file, a wiki doc, a web page) used to be a tab kind too; it now opens in
 * the detail pane's own auxiliary column instead (see detailAux.ts), beside the
 * prose that named it rather than in a tab that hides it.
 *
 * The kind is carried **in the tab id** as a `<kind>:<body>` prefix, not in a
 * parallel map. That is deliberate: `tabGroups.ts` and `sessionTabs.ts` move,
 * reorder, persist and reclaim tabs purely by id, so a kind encoded in the id
 * survives every one of those paths for free — with a side table, each of them
 * would need to remember to keep it in sync, and forgetting would leave a tab
 * that renders as nothing.
 *
 * `DRAFT_TAB_ID` (`new:draft`) established the scheme and its safety argument: a
 * real session id is a UUID, so it can never carry a `<word>:` prefix.
 */

import { DRAFT_TAB_ID } from "./sessionTabs";

export type TabKind =
  | { kind: "session"; sessionId: string }
  | { kind: "sessionview"; sessionId: string }
  | { kind: "draft" };

const SESSIONVIEW_PREFIX = "sessionview:";

/**
 * Tab id for a *second* view of a session already open elsewhere in the column.
 *
 * A tab id is the identity the whole layer dedupes on: opening an id another
 * group holds reveals it there rather than opening a copy, and `tabGroups`
 * invariant 3 forbids the same id in two groups. That is right in general — one
 * thing, one tab — but it is what stops a
 * conversation and the artefacts it produced from sitting side by side, because
 * each `SessionDetail` keeps its *own* view-tab and scroll position. Prefixing
 * the id gives one session two distinct tab identities, so a second pane can
 * exist without any of those reducers learning a special case.
 *
 * The second view is not a different *thing*, only a different *look at* the same
 * thing, which is why nothing else about it is stored: reopen it and you get a
 * fresh pane on 叙事流, then move it to Tokens by hand — the same two clicks that
 * put it there the first time.
 */
export function sessionViewTabId(sessionId: string): string {
  return SESSIONVIEW_PREFIX + sessionId;
}

/** The session a tab is *about*, for the kinds that name one — both views of a
 *  session answer with the same id, which is what lets the list row highlight
 *  and the "already open" marker treat them as one session. */
export function tabSessionId(id: string): string | null {
  const k = parseTabKind(id);
  return k.kind === "session" || k.kind === "sessionview" ? k.sessionId : null;
}

/**
 * Read a tab id's kind.
 *
 * A prefix with an *empty* body (a truncated persisted id, say) falls through to
 * `session` rather than yielding a view tab with no session: an unknown session
 * id simply drops out of the strip, which is a shape the column already handles.
 */
export function parseTabKind(id: string): TabKind {
  if (id === DRAFT_TAB_ID) return { kind: "draft" };
  if (id.length > SESSIONVIEW_PREFIX.length && id.startsWith(SESSIONVIEW_PREFIX)) {
    return { kind: "sessionview", sessionId: id.slice(SESSIONVIEW_PREFIX.length) };
  }
  return { kind: "session", sessionId: id };
}

/**
 * The detail column's "put a second pane beside this one" capability, handed to
 * the session pane.
 *
 * A prop rather than a store hop because it is only meaningful where a tab strip
 * exists: the same `SessionDetail` also renders in the global drawer, which has
 * nowhere to put the extra pane. Everything the agent *cites* (a path, a
 * `[[slug]]`, a url) no longer travels through here — it opens in the pane's own
 * auxiliary column, which every instance has.
 */
export interface DetailTabOpener {
  openSecondView: (sessionId: string) => void;
}

/**
 * Should a restored tab id survive the first-scan prune?
 *
 * Only tabs that *name a session* are prunable, and only because a persisted id
 * can name a session whose transcript has since been deleted — left in the list
 * it would grow forever. A second view is pruned on the same terms as the first:
 * both name the same session, so a deleted transcript must take both with it,
 * not leave the copy behind as an unopenable orphan. The draft names no session
 * and is invisible to the scan, so asking the scan about it would close every
 * restored draft on the first scan after a restart.
 */
export function tabSurvivesScan(
  id: string,
  hasSession: (sessionId: string) => boolean,
): boolean {
  const sid = tabSessionId(id);
  return sid == null ? true : hasSession(sid);
}
