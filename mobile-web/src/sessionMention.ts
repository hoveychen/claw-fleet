// Phone-side pure logic for `@`-mentioning another session in a composer and
// for rendering a sent mention back. The link shape and the summary-block
// parsing are shared with the desktop (`shared-ts/sessionMentions.ts`); this
// file only adds what the phone's plain-text composer and bubble need.

import {
  mentionTitleOf,
  previewWithRefs,
  sessionMentionToken,
  stripRefsBlock,
  type MentionableSession,
} from "../../shared-ts/sessionMentions";
import { splitContextFiles } from "./userAttachments";

/** Longest `@query` still treated as an open mention (mirrors the desktop). */
export const MAX_MENTION_QUERY = 48;

export interface OpenMention {
  /** Index of the `@` in the text. */
  start: number;
  /** What the user typed after the `@`, up to the caret. */
  query: string;
}

/**
 * The `@query` the caret is sitting in, if any: an `@` at the start of the
 * text or right after whitespace, running up to the caret without crossing
 * whitespace. Mirrors the desktop's `detectMention` in useWikiMentions.tsx
 * (separate vite package, so copied rather than imported).
 */
export function detectMention(text: string, caret: number): OpenMention | null {
  for (let i = Math.min(caret, text.length) - 1; i >= 0; i--) {
    const ch = text[i];
    if (/\s/.test(ch)) return null;
    if (ch !== "@") continue;
    const before = i > 0 ? text[i - 1] : "";
    if (before !== "" && !/\s/.test(before)) return null;
    const query = text.slice(i + 1, caret);
    if (query.length > MAX_MENTION_QUERY) return null;
    return { start: i, query };
  }
  return null;
}

/**
 * Replace the open `@query` (from `mention.start` to `caret`) with the link
 * token for `session` plus a trailing space. Returns the new text and where
 * the caret should land (right after the inserted space).
 */
export function applyMention(
  text: string,
  mention: OpenMention,
  caret: number,
  session: MentionableSession,
): { text: string; caret: number } {
  const token = `${sessionMentionToken(mentionTitleOf(session) ?? "", session.id)} `;
  return {
    text: text.slice(0, mention.start) + token + text.slice(caret),
    caret: mention.start + token.length,
  };
}

export type MentionSegment =
  | { kind: "text"; text: string }
  | { kind: "mention"; title: string; id: string };

const MENTION_LINK_RE = /\[@([^\]\n]*)\]\(fleet-session:([A-Za-z0-9_-]+)\)/g;

/** Split a user message into plain runs and `[@title](fleet-session:id)`
 *  mentions, so the bubble can render each mention as a chip. */
export function mentionSegments(text: string): MentionSegment[] {
  const out: MentionSegment[] = [];
  let last = 0;
  for (const m of text.matchAll(MENTION_LINK_RE)) {
    const at = m.index ?? 0;
    if (at > last) out.push({ kind: "text", text: text.slice(last, at) });
    out.push({ kind: "mention", title: m[1], id: m[2] });
    last = at + m[0].length;
  }
  if (last < text.length) out.push({ kind: "text", text: text.slice(last) });
  return out;
}

/** One-line preview of a prompt that may carry mentions and the summary block
 *  Fleet inlined for them: block dropped, links reduced to `@title`. */
export function mentionPreview(text: string): string {
  return previewWithRefs(splitContextFiles(stripRefsBlock(text)).body);
}
