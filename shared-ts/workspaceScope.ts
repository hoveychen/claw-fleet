/** `ws:` scoping for the task-list search box.
 *
 *  The task page's search runs a full-text query over every transcript, so
 *  typing a project's name also pulls in every session elsewhere that merely
 *  *mentions* it. A `ws:<name>` token narrows the list to one workspace strictly
 *  — by the workspace's own name, never by transcript text — and the rest of the
 *  query keeps searching inside that workspace.
 *
 *  The token is the only scope state: the "show only this project" action on a
 *  section heading writes it into the query, and the scope chip's × strips it.
 *  Keeping it in the query string means the existing persisted query carries it
 *  across remounts on both clients with no new storage key.
 *
 *  Shared by the desktop launchpad and the mobile task page so `ws:foo` selects
 *  the same sessions on both. */

/** Matches one `ws:` token: `ws:name` or `ws:"name with spaces"`, anchored at a
 *  word start so `news:x` is not read as a scope. */
const SCOPE_RE = /(^|\s)ws:(?:"([^"]*)"|(\S*))/gi;

export interface ScopedQuery {
  /** The workspace name the query is scoped to, or null when unscoped. */
  workspace: string | null;
  /** The query with every `ws:` token removed — what the text and full-text
   *  search should run on. Whitespace is collapsed. */
  text: string;
}

/** Split a raw search-box query into its workspace scope and remaining text. When
 *  several tokens are given the last non-empty one wins; a bare `ws:` (the user is
 *  still typing) scopes nothing but is still kept out of the text search. */
export function parseWorkspaceScope(query: string): ScopedQuery {
  let workspace: string | null = null;
  const text = query
    .replace(SCOPE_RE, (_m, lead: string, quoted?: string, bare?: string) => {
      const value = (quoted ?? bare ?? "").trim();
      if (value) workspace = value;
      return lead;
    })
    .replace(/\s+/g, " ")
    .trim();
  return { workspace, text };
}

/** The `ws:` token for a workspace name, quoted when it holds whitespace or a
 *  quote would otherwise be needed to keep it one token. */
export function workspaceScopeToken(name: string): string {
  return /[\s"]/.test(name) ? `ws:"${name.replace(/"/g, "")}"` : `ws:${name}`;
}

/** Replace any scope in `query` with `name` (or drop it when `name` is null),
 *  keeping the user's remaining search text after the token. */
export function withWorkspaceScope(query: string, name: string | null): string {
  const { text } = parseWorkspaceScope(query);
  if (!name) return text;
  return text ? `${workspaceScopeToken(name)} ${text}` : `${workspaceScopeToken(name)} `;
}

/** Keep the sessions in workspace `scope`. A name that some session carries
 *  exactly (case-insensitive) selects only that workspace — the heading action
 *  writes exact names, and `claude-fleet` must not also pull in
 *  `claude-fleet-cloud`. Otherwise the scope is a substring, so a hand-typed
 *  `ws:zoo` still finds `human-zoo`. */
export function filterByWorkspaceScope<T extends { workspaceName: string }>(
  sessions: T[],
  scope: string | null,
): T[] {
  if (!scope) return sessions;
  const needle = scope.toLowerCase();
  const exact = sessions.some((s) => s.workspaceName.toLowerCase() === needle);
  return sessions.filter((s) => {
    const name = s.workspaceName.toLowerCase();
    return exact ? name === needle : name.includes(needle);
  });
}
