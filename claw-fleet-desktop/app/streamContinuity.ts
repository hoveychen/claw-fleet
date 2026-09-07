import type { LiveThinking } from "./types";

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
