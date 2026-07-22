import { createContext, useContext, type ReactNode } from "react";

/**
 * Navigating from an `Agent` tool card (or a Workflow agent row) into that
 * subagent's own transcript. Ported from the desktop AgentNavContext.
 *
 * Passed by context rather than by prop: the tool card sits several levels below
 * SessionDetailView, behind the `MessageRow` `memo` boundary — a context
 * consumer re-renders even inside `memo`, so a threaded callback buys nothing.
 *
 * A subagent's transcript lives at `<session>/subagents/agent-<agentId>.jsonl`
 * and the core scanner registers it under the session id `agent-<agentId>`
 * (mobile_relay keeps those rows in the snapshot for parents that survived the
 * cap). `open`/`has` therefore just look `agent-<agentId>` up in the session
 * array — the same table-lookup model the desktop uses.
 */
export interface AgentNav {
  /** Open the subagent's transcript (pushes a detail layer). */
  open: (agentId: string) => void;
  /**
   * Whether a scanned session exists for this agent. The card asks first, so a
   * subagent the snapshot hasn't surfaced yet shows no link rather than a button
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
