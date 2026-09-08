// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";

import { openDoc, showFacet, type AuxState } from "./detailAux";
import { useSessionAux } from "./useSessionAux";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement | null = null;
let root: Root | null = null;

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
});

/** Mounts the hook once and re-renders it at whatever session id we point it
 *  at — the same shape as SessionDetail, which is never remounted. */
function mount(sessionId: string) {
  const seen: { state: AuxState; set: (u: (s: AuxState) => AuxState) => void } = {
    state: null as unknown as AuxState,
    set: () => {},
  };
  function Probe({ id }: { id: string }) {
    const [aux, setAux] = useSessionAux(id);
    seen.state = aux;
    seen.set = (u) => setAux(u);
    return null;
  }
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(<Probe id={sessionId} />));
  return {
    probe: seen,
    switchTo: (id: string) => act(() => root!.render(<Probe id={id} />)),
  };
}

describe("useSessionAux", () => {
  it("keeps what the reader opened while the session stays the same", () => {
    const { probe } = mount("s1");

    act(() => probe.set((st) => openDoc(st, "file", "/repo/src/main.rs")));
    expect(probe.state.docs).toHaveLength(1);
  });

  // The regression this hook exists for: SessionDetail is re-pointed, not
  // remounted, so without a per-session reset the previous conversation's doc
  // cards sit in the next one's rail beside a transcript that never named them.
  it("hands the next session a clean rail and a closed drawer", () => {
    const { probe, switchTo } = mount("s1");
    act(() => probe.set((st) => openDoc(st, "file", "/repo/src/main.rs")));
    act(() => probe.set((st) => showFacet(st, "tokens")));

    switchTo("s2");

    expect(probe.state.docs).toEqual([]);
    expect(probe.state.active).toBe(null);
    expect(probe.state.expanded).toBe(null);
  });
});
