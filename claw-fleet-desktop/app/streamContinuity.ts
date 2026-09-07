import type { LiveThinking, RawMessage } from "./types";

/**
 * Keep visible reasoning mounted through an empty sample from a growing
 * sidecar — but only for the session we are actually showing.
 *
 * The retain-through-empty rule is what makes the sidecar/transcript handoff
 * seamless, and it is also what leaked reasoning across sessions: SessionDetail
 * is not remounted on a session switch, and a session with no sidecar at all
 * samples back exactly like a session between two chunks. `sessionId` is the
 * discriminator that tells those two apart, so a retained block is dropped the
 * moment it no longer belongs to the open session, and a sample that raced in
 * for the session we just left is ignored rather than shown under the new one.
 */
export function retainLiveThinking(
  previous: LiveThinking | null,
  incoming: LiveThinking | null,
  sessionId: string,
): LiveThinking | null {
  const retained = previous && previous.sessionId === sessionId ? previous : null;
  if (incoming !== null && incoming.sessionId !== sessionId) return retained;
  if (incoming === null || (incoming.streaming && incoming.thinking.length === 0)) {
    return retained;
  }
  return incoming;
}

/** True once the durable transcript contains the live reasoning snapshot. */
export function liveThinkingLanded(messages: RawMessage[], live: LiveThinking): boolean {
  for (let i = messages.length - 1; i >= Math.max(0, messages.length - 4); i -= 1) {
    const content = messages[i]?.message?.content;
    if (!Array.isArray(content)) continue;
    for (const block of content) {
      if (block.type !== "thinking") continue;
      const settled = (block as { thinking?: string }).thinking ?? "";
      if (settled.includes(live.thinking)) return true;
    }
  }
  return false;
}

export type FollowGrowthBehavior = ScrollBehavior | null;

/** Initial layout pins instantly; subsequent content growth is visibly followed. */
export function followGrowthBehavior(
  previousHeight: number | null,
  nextHeight: number,
): FollowGrowthBehavior {
  if (previousHeight === null) return "instant";
  if (nextHeight === previousHeight) return null;
  return nextHeight > previousHeight ? "smooth" : "instant";
}
