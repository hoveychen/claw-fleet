import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { HandoffChain, RawMessage, SessionInfo } from "./types";
import { INITIAL_TAIL, LOAD_EARLIER_STEP, useSessionsStore } from "./store";
import {
  nextLeg,
  prependedCount,
  stitchLegs,
  type LoadedLeg,
} from "../../shared-ts/prevLeg";

/** A prepended relay leg plus the paging state of its own transcript. */
interface PrevLeg extends LoadedLeg<RawMessage> {
  jsonlPath: string;
  tail: number;
  fullyLoaded: boolean;
}

/** The "load leg N" control, when one is on offer. */
export interface PrevLegOffer {
  hop: number;
  loading: boolean;
  onLoad: () => Promise<void>;
}

interface CurrentLeg {
  session: SessionInfo | null;
  messages: RawMessage[];
  fullyLoaded: boolean;
  loadEarlier: () => Promise<void>;
  isLoadingEarlier: boolean;
}

/**
 * Walk a relay chain backwards from the open leg: once the current leg is
 * paged to its first message, offer its predecessor; load that leg's tail
 * above it; page it with the same "load earlier" steps; once it too is fully
 * loaded, offer the leg before it — down to leg 1.
 *
 * Works on top of whichever mode SessionDetail runs in (global store or
 * standalone mirror): it reads the current leg's resolved state and returns
 * the stitched view for `MessageList` only. Prepended legs are read-only —
 * the composer, resume, live-follow tail and optimistic echo never see them.
 */
export function usePrevLegs(current: CurrentLeg) {
  const { session } = current;
  const sessions = useSessionsStore((s) => s.sessions);
  const [chain, setChain] = useState<HandoffChain | null>(null);
  const [legs, setLegs] = useState<PrevLeg[]>([]);
  const [loading, setLoading] = useState(false);
  // Bumped on every reset, so a fetch that lands after the pane switched
  // session is dropped instead of grafting one chain's legs onto another.
  const epoch = useRef(0);

  const sessionId = session?.id ?? null;
  const jsonlPath = session?.jsonlPath ?? null;
  const hop = session?.handoff?.hop ?? 0;
  const chainId = session?.handoff?.chainId ?? null;

  useEffect(() => {
    epoch.current += 1;
    setChain(null);
    setLegs([]);
    setLoading(false);
  }, [sessionId, jsonlPath]);

  // The chain is only needed once a button could appear: read it the first
  // time the current leg reaches its start.
  const wantChain = hop > 1 && current.fullyLoaded && chain === null && !!sessionId;
  useEffect(() => {
    if (!wantChain || !sessionId) return;
    const mine = epoch.current;
    invoke<HandoffChain | null>("get_handoff_chain", { sessionId })
      .then((c) => {
        if (epoch.current === mine && c && c.chainId) setChain(c);
      })
      .catch((e) => console.error("Failed to read the handoff chain:", e));
  }, [wantChain, sessionId, chainId]);

  const top = legs.length > 0 ? legs[legs.length - 1] : null;
  const topId = top ? top.sessionId : sessionId;
  const topHop = top ? top.hop : hop;
  const topFullyLoaded = top ? top.fullyLoaded : current.fullyLoaded;

  const next = topId
    ? nextLeg({ links: chain?.links ?? null, topId, topHop, topFullyLoaded, sessions })
    : null;

  const loadPrev = useCallback(async () => {
    if (!next || loading) return;
    const { link, pred, hop: legHop } = next;
    const mine = epoch.current;
    setLoading(true);
    try {
      const msgs = await invoke<RawMessage[]>("get_messages_tail", {
        jsonlPath: pred.jsonlPath,
        tail: INITIAL_TAIL,
      });
      if (epoch.current !== mine) return;
      setLegs((prev) => [
        ...prev,
        {
          sessionId: pred.id,
          hop: legHop,
          title: pred.titleOverride || pred.aiTitle || `${pred.id.slice(0, 8)}…`,
          handedAt: link.handedAt,
          jsonlPath: pred.jsonlPath,
          messages: msgs,
          tail: INITIAL_TAIL,
          fullyLoaded: msgs.length < INITIAL_TAIL,
        },
      ]);
    } catch (e) {
      console.error("Failed to load the previous relay leg:", e);
    } finally {
      if (epoch.current === mine) setLoading(false);
    }
  }, [next?.link, next?.pred, next?.hop, loading]);

  /** Page the topmost prepended leg one step further back. */
  const loadEarlierTop = useCallback(async () => {
    if (!top || top.fullyLoaded || loading) return;
    const mine = epoch.current;
    const nextTail = top.tail + LOAD_EARLIER_STEP;
    setLoading(true);
    try {
      const msgs = await invoke<RawMessage[]>("get_messages_tail", {
        jsonlPath: top.jsonlPath,
        tail: nextTail,
      });
      if (epoch.current !== mine) return;
      setLegs((prev) =>
        prev.map((l) =>
          l.sessionId === top.sessionId
            ? { ...l, messages: msgs, tail: nextTail, fullyLoaded: msgs.length < nextTail }
            : l,
        ),
      );
    } finally {
      if (epoch.current === mine) setLoading(false);
    }
  }, [top, loading]);

  const messages = useMemo(
    () => stitchLegs(legs, current.messages) as RawMessage[],
    [legs, current.messages],
  );

  // A prepended leg's truncated tool output lives in *its* transcript.
  const toolResultPaths = useMemo(() => {
    const m = new Map<string, string>();
    for (const l of legs) {
      for (const msg of l.messages) {
        const content = msg.message?.content;
        if (!msg._fleetTruncated || !Array.isArray(content)) continue;
        for (const b of content) {
          if (b.type === "tool_result") m.set((b as { tool_use_id: string }).tool_use_id, l.jsonlPath);
        }
      }
    }
    return m;
  }, [legs]);

  const offer: PrevLegOffer | null = next ? { hop: next.hop, loading, onLoad: loadPrev } : null;

  return {
    messages,
    readOnlyPrefix: prependedCount(legs),
    toolResultPaths,
    fullyLoaded: top ? top.fullyLoaded : current.fullyLoaded,
    loadEarlier: top ? loadEarlierTop : current.loadEarlier,
    isLoadingEarlier: top ? loading : current.isLoadingEarlier,
    offer,
  };
}
