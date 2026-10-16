/**
 * `@`-mentions of another session (or the relay chain it sits on), shared by
 * the desktop composers and the phone composer.
 *
 * A mention is a markdown link with the `fleet-session:` scheme inserted into
 * the prose: `[@title](fleet-session:<id>)`. The core side
 * (`claw-fleet-core/src/session_refs.rs`) parses exactly this shape, inlines a
 * summary of each referenced chain as a `<fleet-referenced-sessions>` block and
 * grants the receiving session history access to it. Both halves must agree on
 * the shape, so it lives here once for the front ends.
 */

export const SESSION_MENTION_SCHEME = "fleet-session:";

const BLOCK_RE = /\n*<fleet-referenced-sessions>[\s\S]*?<\/fleet-referenced-sessions>/;

/** The fields a mention picker needs from a session row. */
export interface MentionableSession {
  id: string;
  titleOverride?: string | null;
  aiTitle?: string | null;
  slug?: string | null;
  workspaceName?: string;
  lastActivityMs: number;
  isSubagent?: boolean;
  handoff?: { chainId: string; hop: number; chainLen: number } | null;
}

/** Display title of a session for a mention, or `null` when it has none. */
export function mentionTitleOf(s: MentionableSession): string | null {
  return s.titleOverride || s.aiTitle || s.slug || null;
}

/** The token a picked session is inserted as. Brackets in the title are
 *  swapped for full-width ones so they cannot end the link text early. */
export function sessionMentionToken(title: string, sessionId: string): string {
  const safe = title.replace(/\[/g, "［").replace(/\]/g, "］").replace(/\s+/g, " ").trim();
  return `[@${safe || sessionId.slice(0, 8)}](${SESSION_MENTION_SCHEME}${sessionId})`;
}

/** Session id named by a mention link's href, or `null` for any other href. */
export function sessionIdFromHref(href: string | null | undefined): string | null {
  if (!href || !href.startsWith(SESSION_MENTION_SCHEME)) return null;
  const id = href.slice(SESSION_MENTION_SCHEME.length);
  return /^[A-Za-z0-9_-]+$/.test(id) ? id : null;
}

/** Split off the summary block Fleet inlined after a mention. `refs` is how
 *  many referenced sessions the block summarised (its `## ` headings). */
export function splitRefsBlock(text: string): { text: string; refs: number } {
  const m = BLOCK_RE.exec(text);
  if (!m) return { text, refs: 0 };
  const refs = (m[0].match(/^## /gm) ?? []).length;
  return { text: text.slice(0, m.index) + text.slice(m.index + m[0].length), refs };
}

/** `text` without the inlined summary block (previews, queued chips). */
export function stripRefsBlock(text: string): string {
  return splitRefsBlock(text).text;
}

/** Replace each mention link with its plain `@title` (one-line previews). */
export function plainMentions(text: string): string {
  return text.replace(/\[@([^\]\n]*)\]\(fleet-session:[A-Za-z0-9_-]+\)/g, "@$1");
}

/**
 * Sessions a picker offers for `query`: one row per relay chain (its latest
 * hop, which is what "that chain" means to the user), no subagents, none in
 * `exclude`, matched on title / workspace / id prefix, most recent first.
 */
export function pickMentionableSessions<T extends MentionableSession>(
  sessions: T[],
  query: string,
  opts: { exclude?: string[]; limit?: number } = {},
): T[] {
  const exclude = new Set(opts.exclude ?? []);
  const latestByChain = new Map<string, T>();
  const singles: T[] = [];
  for (const s of sessions) {
    if (s.isSubagent) continue;
    const chain = s.handoff?.chainId;
    if (!chain) {
      singles.push(s);
      continue;
    }
    const cur = latestByChain.get(chain);
    if (!cur || (s.handoff!.hop > cur.handoff!.hop)) latestByChain.set(chain, s);
  }
  // A chain is excluded as a whole when any of its hops is (the receiving
  // session referencing its own chain is noise).
  const excludedChains = new Set(
    sessions.filter((s) => exclude.has(s.id) && s.handoff?.chainId).map((s) => s.handoff!.chainId),
  );
  const q = query.trim().toLowerCase();
  const rows = [...singles, ...latestByChain.values()].filter((s) => {
    if (exclude.has(s.id)) return false;
    if (s.handoff?.chainId && excludedChains.has(s.handoff.chainId)) return false;
    if (!mentionTitleOf(s)) return false;
    if (!q) return true;
    return (
      (mentionTitleOf(s) ?? "").toLowerCase().includes(q) ||
      (s.workspaceName ?? "").toLowerCase().includes(q) ||
      s.id.toLowerCase().startsWith(q)
    );
  });
  rows.sort((a, b) => b.lastActivityMs - a.lastActivityMs);
  return rows.slice(0, opts.limit ?? 8);
}
