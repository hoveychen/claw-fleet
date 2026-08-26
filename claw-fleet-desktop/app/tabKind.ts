/**
 * What a detail-column tab *holds*.
 *
 * Before this, a tab was always one session (plus the one synthetic
 * `new:draft`). An IDE's tab strip holds whatever you were reading, so a tab can
 * now also be a repo file, a wiki doc, or a web page — which is what lets a doc
 * sit open beside the prose that referenced it instead of replacing the page.
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
 *
 * The body is never re-split, so a Windows path keeps its drive-letter colon and
 * a URL keeps its scheme.
 */

import { DRAFT_TAB_ID } from "./sessionTabs";

export type TabKind =
  | { kind: "session"; sessionId: string }
  | { kind: "sessionview"; sessionId: string }
  | { kind: "draft" }
  | { kind: "file"; absPath: string }
  | { kind: "wiki"; slug: string }
  | { kind: "web"; url: string };

const FILE_PREFIX = "file:";
const WIKI_PREFIX = "wiki:";
const WEB_PREFIX = "web:";
const SESSIONVIEW_PREFIX = "sessionview:";

/**
 * Tab id for a repo file. Keyed on the path alone — the line to scroll to is
 * *not* part of the identity, so clicking `foo.rs:10` and then `foo.rs:99`
 * reveals one tab twice rather than opening two. The line travels beside the id
 * (see HistoryView's `fileLineById`), the same way the per-tab search highlight
 * does.
 */
export function fileTabId(absPath: string): string {
  return FILE_PREFIX + absPath;
}

/** Tab id for a wiki doc. `slug` may contain `/` (virtual directories). */
export function wikiTabId(slug: string): string {
  return WIKI_PREFIX + slug;
}

/** Tab id for a web page. */
export function webTabId(url: string): string {
  return WEB_PREFIX + url;
}

/**
 * Tab id for a *second* view of a session already open elsewhere in the column.
 *
 * A tab id is the identity the whole layer dedupes on: `openTabRouted` reveals an
 * id another group holds rather than opening a copy, and `tabGroups` invariant 3
 * forbids the same id in two groups. That is exactly right for a file — two tabs
 * of one path would show the same bytes twice — but it is what stops a
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
 * `session` rather than yielding a file tab with no path: an unknown session id
 * simply drops out of the strip, which is a shape the column already handles,
 * whereas a pathless file tab would render a permanent error box.
 */
export function parseTabKind(id: string): TabKind {
  if (id === DRAFT_TAB_ID) return { kind: "draft" };
  if (id.length > FILE_PREFIX.length && id.startsWith(FILE_PREFIX)) {
    return { kind: "file", absPath: id.slice(FILE_PREFIX.length) };
  }
  if (id.length > WIKI_PREFIX.length && id.startsWith(WIKI_PREFIX)) {
    return { kind: "wiki", slug: id.slice(WIKI_PREFIX.length) };
  }
  if (id.length > WEB_PREFIX.length && id.startsWith(WEB_PREFIX)) {
    return { kind: "web", url: id.slice(WEB_PREFIX.length) };
  }
  if (id.length > SESSIONVIEW_PREFIX.length && id.startsWith(SESSIONVIEW_PREFIX)) {
    return { kind: "sessionview", sessionId: id.slice(SESSIONVIEW_PREFIX.length) };
  }
  return { kind: "session", sessionId: id };
}

/**
 * The detail column's "open this beside what I'm reading" capability, handed
 * down to the surfaces that render agent prose.
 *
 * It is a prop rather than a store hop because it is only meaningful where a tab
 * strip exists: the same `SessionDetail` also renders in the global drawer and
 * in Lite mode, and there a clicked path still belongs in the 仓库 page. A
 * surface holding one of these knows it has somewhere to put a tab; a surface
 * without it keeps the page-switching behaviour.
 */
export interface DetailTabOpener {
  /** `line` is accepted (and currently unused) because the caller has it: no
   *  renderer in the app anchors to a line yet — the 仓库 page has carried the
   *  same field, equally unread, since paths became clickable. */
  openFile: (absPath: string, line: number | null) => void;
  openWiki: (slug: string) => void;
  openWeb: (url: string) => void;
  /** A second pane on the *same* session, beside the first — the move that lets
   *  one half stay on the conversation while the other sits on Tokens or 计划.
   *  Lives here because it is the same capability as the three above: it is only
   *  meaningful where a tab strip exists to hold the extra pane. */
  openSecondView: (sessionId: string) => void;
}

/**
 * Should a restored tab id survive the first-scan prune?
 *
 * Only tabs that *name a session* are prunable, and only because a persisted id
 * can name a session whose transcript has since been deleted — left in the list
 * it would grow forever. A second view is pruned on the same terms as the first:
 * both name the same session, so a deleted transcript must take both with it,
 * not leave the copy behind as an unopenable orphan. Every other kind is
 * invisible to the session scan (a file, a wiki doc and a web page are not
 * sessions), so asking the scan about them would silently close every restored
 * one on the first scan after a restart.
 */
export function tabSurvivesScan(
  id: string,
  hasSession: (sessionId: string) => boolean,
): boolean {
  const sid = tabSessionId(id);
  return sid == null ? true : hasSession(sid);
}

/** Last path segment, tolerating either separator so a Windows path reads the
 *  same as a POSIX one. */
function basename(p: string): string {
  const cut = Math.max(p.lastIndexOf("/"), p.lastIndexOf("\\"));
  return cut >= 0 ? p.slice(cut + 1) : p;
}

/**
 * The tab's name, when the id alone determines it.
 *
 * Returns `null` for session (either view of one) and draft tabs: a session's
 * title comes from the live scan (so a ten-minute-old tab shows the agent's
 * *current* title) and the draft's from i18n. Neither is derivable here.
 */
export function tabKindLabel(k: TabKind): string | null {
  switch (k.kind) {
    case "file":
      return basename(k.absPath);
    case "wiki":
      return basename(k.slug);
    case "web":
      try {
        return new URL(k.url).host;
      } catch {
        // A hand-typed or malformed url still deserves a readable tab.
        return k.url;
      }
    default:
      return null;
  }
}
