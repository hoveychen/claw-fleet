// Where an expanded rail card's reader is drawn.
//
// The rail is a column beside the conversation, too narrow to read a file or a
// transcript in. So a card that expands keeps its row in the rail (marked open)
// and portals its reader into a host that floats over the conversation's right
// edge — directly left of the rail, below the header, clear of the rail's own
// scroller (which would otherwise clip it). SessionDetail owns the host element
// and provides it here; with no host (a component test, say) the reader renders
// in place.

import { createContext, useContext, type ReactNode } from "react";
import { createPortal } from "react-dom";

export const ReaderHostContext = createContext<HTMLElement | null>(null);

export function InReaderHost({ children }: { children: ReactNode }) {
  const host = useContext(ReaderHostContext);
  return host ? createPortal(children, host) : <>{children}</>;
}
