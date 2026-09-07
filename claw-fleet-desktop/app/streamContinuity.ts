import type { LiveThinking, RawMessage } from "./types";

/** Keep visible reasoning mounted through an empty sample from a growing sidecar. */
export function retainLiveThinking(
  previous: LiveThinking | null,
  incoming: LiveThinking | null,
): LiveThinking | null {
  if (incoming === null || (incoming.streaming && incoming.thinking.length === 0)) {
    return previous;
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
