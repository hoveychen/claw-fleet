import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { FleetTransport } from "../transport";
import type { HandoffChain, RawMessage, SessionInfo } from "../types";
import { hasSuccessor, legTitle, nextLeg, successorLeg, type LoadedLeg } from "../../../shared-ts/prevLeg";

/** Raw lines fetched per page of a prepended leg — the same steps the
 *  current leg's own "加载更早的消息" uses. */
export const LEG_TAIL_INITIAL = 120;
export const LEG_TAIL_STEP = 200;

/** A prepended relay leg plus its own paging state. */
export interface PrevLeg extends LoadedLeg<RawMessage> {
  jsonlPath: string;
  agentSource?: string | null;
  n: number;
}

/**
 * Walk a relay chain backwards from the open leg (mobile counterpart of the
 * desktop `usePrevLegs`): once the current leg has no earlier lines, offer its
 * predecessor; once that leg is paged to its start, offer the one before it.
 *
 * Legs are kept nearest-first, as loaded; the caller renders them oldest-first
 * above the current transcript, each opened by its heading (see `stitchLegs`). They are display-only — the
 * tail poller, composer and optimistic echo never see them, and nothing here
 * polls: a predecessor leg has finished by definition.
 */
export function usePrevLegs(
  client: FleetTransport | null,
  session: SessionInfo,
  sessions: SessionInfo[],
  currentFullyLoaded: boolean,
) {
  const [chain, setChain] = useState<HandoffChain | null>(null);
  const [legs, setLegs] = useState<PrevLeg[]>([]);
  const [loading, setLoading] = useState(false);
  // Bumped on every reset so a reply that lands after the view switched
  // session is dropped instead of grafting one chain's legs onto another.
  const epoch = useRef(0);
  const hop = session.handoff?.hop ?? 0;

  useEffect(() => {
    epoch.current += 1;
    setChain(null);
    setLegs([]);
    setLoading(false);
  }, [session.id, session.jsonlPath]);

  const wantChain = !!client && hop > 1 && currentFullyLoaded && chain === null;
  useEffect(() => {
    if (!wantChain || !client) return;
    const mine = epoch.current;
    client
      .request<HandoffChain | null>("handoff_chain", { sessionId: session.id })
      .then((c) => {
        // The relay resolves a null reply to an empty object; a real chain
        // carries a chainId.
        if (epoch.current === mine && c && c.chainId) setChain(c);
      })
      .catch(() => {});
  }, [wantChain, client, session.id]);

  const top = legs.length > 0 ? legs[legs.length - 1] : null;
  const topId = top ? top.sessionId : session.id;
  const topHop = top ? top.hop : hop;
  const topFullyLoaded = top ? top.fullyLoaded : currentFullyLoaded;
  const next = nextLeg({ links: chain?.links ?? null, topId, topHop, topFullyLoaded, sessions });

  const loadPrev = useCallback(async () => {
    if (!client || !next || loading) return;
    const { pred, hop: legHop } = next;
    const mine = epoch.current;
    setLoading(true);
    try {
      const rows = await client.request<RawMessage[]>("tail", {
        path: pred.jsonlPath,
        n: LEG_TAIL_INITIAL,
      });
      if (epoch.current !== mine) return;
      setLegs((prev) => [
        ...prev,
        {
          sessionId: pred.id,
          hop: legHop,
          title: legTitle(pred),
          createdAtMs: pred.createdAtMs,
          jsonlPath: pred.jsonlPath,
          agentSource: pred.agentSource,
          messages: rows,
          n: LEG_TAIL_INITIAL,
          // A short tail means the transcript has no lines before it.
          fullyLoaded: rows.length < LEG_TAIL_INITIAL,
        },
      ]);
    } catch {
      // The button stays; the next tap retries.
    } finally {
      if (epoch.current === mine) setLoading(false);
    }
  }, [client, next?.link, next?.pred, next?.hop, loading]);

  /** Page the topmost prepended leg one step further back. */
  const loadEarlierTop = useCallback(async () => {
    if (!client || !top || top.fullyLoaded || loading) return;
    const mine = epoch.current;
    const n = top.n + LEG_TAIL_STEP;
    setLoading(true);
    try {
      const rows = await client.request<RawMessage[]>("tail", { path: top.jsonlPath, n });
      if (epoch.current !== mine) return;
      setLegs((prev) =>
        prev.map((l) =>
          l.sessionId === top.sessionId ? { ...l, messages: rows, n, fullyLoaded: rows.length < n } : l,
        ),
      );
    } catch {
      // Same as above: tap again.
    } finally {
      if (epoch.current === mine) setLoading(false);
    }
  }, [client, top, loading]);

  return {
    legs,
    loading,
    /** "加载第 N 棒", when the leg on top is complete and has a predecessor. */
    offer: next ? { hop: next.hop, onLoad: loadPrev } : null,
    /** Paging for the topmost prepended leg, while it has earlier lines. */
    earlier: top && !top.fullyLoaded ? loadEarlierTop : null,
  };
}

/**
 * The open session's successor on its relay chain, or null while it has none.
 * A view left open across a handoff otherwise ends on the last reply with
 * nothing pointing at where the work went. Re-reads the chain whenever the
 * snapshot reports it grew.
 */
export function useSuccessorLeg(
  client: FleetTransport | null,
  session: SessionInfo,
  sessions: SessionInfo[],
): { sessionId: string; hop: number; title: string } | null {
  const handoff = session.handoff ?? null;
  const want = !!client && hasSuccessor(handoff);
  const chainLen = handoff?.chainLen ?? 0;
  const [chain, setChain] = useState<{ sessionId: string; chain: HandoffChain } | null>(null);

  useEffect(() => {
    if (!want || !client) return;
    let live = true;
    const sessionId = session.id;
    client
      .request<HandoffChain | null>("handoff_chain", { sessionId })
      .then((c) => {
        // The relay resolves a null reply to an empty object; a real chain
        // carries a chainId.
        if (live && c && c.chainId) setChain({ sessionId, chain: c });
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [want, client, session.id, chainLen]);

  return useMemo(() => {
    if (!want || !handoff || chain?.sessionId !== session.id) return null;
    return successorLeg({ links: chain.chain.links, sessionId: session.id, hop: handoff.hop, sessions });
  }, [want, handoff, chain, session.id, sessions]);
}
