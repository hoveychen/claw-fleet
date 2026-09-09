import { createContext, useContext, type ReactNode } from "react";
import type { AuxDocKind } from "../../detailAux";

/**
 * Ambient "open this ingest" for a whole transcript.
 *
 * Same problem and the same answer as `WikiLinksProvider`: between
 * `SessionDetail`, which owns the auxiliary rail, and the `FleetToolCard` that
 * finally draws an ingest preview sit MessageList, MessageRow, WorkRunBlock and
 * ContentBlocks. Threading a capability prop through all of them means every
 * future block renderer has to remember to forward it, and forgetting fails
 * silently. So the transcript provides it once, at the top.
 *
 * Outside any provider — the mock board, a unit test, any surface with no rail
 * to open into — the value is `null`, and the card falls back to navigating
 * straight to the 产出 / 知识库 page. That fallback is why this is a context and
 * not a required prop: a preview card with a dead click would be worse than one
 * that always jumps.
 */
export interface IngestOpenContext {
  /** Card it in the rail and expand it into a reader. */
  open: (kind: AuxDocKind, ref: string, label: string) => void;
  /** The rail's currently expanded doc id (`kind:ref`), or null. */
  expandedId: string | null;
}

const IngestOpenCtx = createContext<IngestOpenContext | null>(null);

export function IngestOpenProvider({
  value,
  children,
}: {
  value: IngestOpenContext | null;
  children: ReactNode;
}) {
  return <IngestOpenCtx.Provider value={value}>{children}</IngestOpenCtx.Provider>;
}

export function useIngestOpen(): IngestOpenContext | null {
  return useContext(IngestOpenCtx);
}
