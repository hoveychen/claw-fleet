import { createContext, useContext, type ReactNode } from "react";

/**
 * Navigating from an `Agent` tool card into that subagent's own transcript.
 *
 * Passed by context rather than by prop: the card sits five levels below
 * `SessionDetail`, behind two `memo` boundaries, and threading a callback
 * through them buys nothing.
 *
 * A subagent's transcript lives at `<session>/subagents/agent-<agentId>.jsonl`
 * and the scanner registers it under the session id `agent-<agentId>` — verified
 * against all 57 `Agent` calls in a 120-transcript sample.
 */
export interface AgentNav {
  /** Open the subagent's transcript. */
  open: (agentId: string) => void;
  /**
   * Whether a scanned session exists for this agent. The card asks first, so a
   * subagent the scan hasn't surfaced yet shows no link rather than a button
   * that silently does nothing.
   */
  has: (agentId: string) => boolean;
}

const AgentNavContext = createContext<AgentNav | null>(null);

export function AgentNavProvider({ nav, children }: { nav: AgentNav; children: ReactNode }) {
  return <AgentNavContext.Provider value={nav}>{children}</AgentNavContext.Provider>;
}

/** Null outside a provider, so a card rendered elsewhere simply omits the link. */
export function useAgentNav(): AgentNav | null {
  return useContext(AgentNavContext);
}
