import { createContext, useContext, type ReactNode } from "react";
import type { WikiLinkContext } from "./wikiLinks";

/**
 * Ambient wiki-link resolution for a whole subtree of markdown.
 *
 * `[[slug]]` refs need two things a text block cannot know on its own: whether
 * the slug is published, and where to open it. The 知识库 page hands that down as
 * a prop, because it renders one doc. Agent prose can't: between `SessionDetail`
 * and the `TextBlock` that finally draws a paragraph sit MessageList, MessageRow,
 * ContentBlocks, UserContent, the reader modal and the tool-block renderers —
 * threading a second capability prop through all of them (beside the path one
 * already there) would mean every future block renderer has to remember to
 * forward it, and forgetting silently turns refs back into plain text.
 *
 * So the transcript provides it once, at the top. An explicit `wiki` prop still
 * wins where a caller has one.
 */
const WikiLinksCtx = createContext<WikiLinkContext | null>(null);

export function WikiLinksProvider({
  value,
  children,
}: {
  value: WikiLinkContext | null;
  children: ReactNode;
}) {
  return <WikiLinksCtx.Provider value={value}>{children}</WikiLinksCtx.Provider>;
}

/** The ambient context, or `null` outside any provider — in which case a
 *  `[[slug]]` stays prose rather than becoming a link to nowhere. */
export function useWikiLinks(): WikiLinkContext | null {
  return useContext(WikiLinksCtx);
}
