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
  | { kind: "draft" }
  | { kind: "file"; absPath: string }
  | { kind: "wiki"; slug: string }
  | { kind: "web"; url: string };

const FILE_PREFIX = "file:";
const WIKI_PREFIX = "wiki:";
const WEB_PREFIX = "web:";

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
  return { kind: "session", sessionId: id };
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
 * Returns `null` for session and draft tabs: a session's title comes from the
 * live scan (so a ten-minute-old tab shows the agent's *current* title) and the
 * draft's from i18n. Neither is derivable here.
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
