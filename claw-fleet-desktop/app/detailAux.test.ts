import { describe, expect, it } from "vitest";
import {
  auxDocLabel,
  auxVisible,
  closeAux,
  closeDoc,
  docId,
  initialAux,
  MAX_AUX_DOCS,
  openDoc,
  pruneFacet,
  syncLiveAgents,
  toggleFacet,
  type AuxState,
} from "./detailAux";

describe("toggleFacet", () => {
  it("opens a facet", () => {
    expect(toggleFacet(initialAux, "tokens").active).toBe("tokens");
  });

  it("clicking the showing facet closes the panel", () => {
    const open = toggleFacet(initialAux, "tokens");
    expect(toggleFacet(open, "tokens").active).toBe(null);
  });

  it("switching facets keeps the panel open", () => {
    const open = toggleFacet(initialAux, "tokens");
    expect(toggleFacet(open, "skills").active).toBe("skills");
  });

  it("clears a previous dismissal so the panel actually reopens", () => {
    const dismissed = closeAux(initialAux);
    expect(dismissed.agentsDismissed).toBe(true);
    expect(toggleFacet(dismissed, "skills").agentsDismissed).toBe(false);
  });
});

describe("openDoc", () => {
  it("opens and focuses a file", () => {
    const st = openDoc(initialAux, "file", "/repo/src/main.rs");
    expect(st.docs).toHaveLength(1);
    expect(st.active).toBe(docId("file", "/repo/src/main.rs"));
    expect(st.docs[0].label).toBe("main.rs");
  });

  it("reveals rather than duplicates an already-open doc", () => {
    let st = openDoc(initialAux, "wiki", "arch/overview");
    st = toggleFacet(st, "tokens");
    st = openDoc(st, "wiki", "arch/overview");
    expect(st.docs).toHaveLength(1);
    expect(st.active).toBe(docId("wiki", "arch/overview"));
  });

  it("trims the oldest doc past the cap and keeps the newest focused", () => {
    let st: AuxState = initialAux;
    for (let i = 0; i < MAX_AUX_DOCS + 2; i += 1) {
      st = openDoc(st, "file", `/repo/f${i}.rs`);
    }
    expect(st.docs).toHaveLength(MAX_AUX_DOCS);
    expect(st.docs[0].ref).toBe("/repo/f2.rs");
    expect(st.active).toBe(docId("file", `/repo/f${MAX_AUX_DOCS + 1}.rs`));
  });

  it("a doc id can never collide with a facet name", () => {
    const st = openDoc(initialAux, "file", "tokens");
    expect(st.active).not.toBe("tokens");
  });
});

describe("closeDoc", () => {
  it("falls back to the neighbour when closing the focused doc", () => {
    let st = openDoc(initialAux, "file", "/a.rs");
    st = openDoc(st, "file", "/b.rs");
    st = closeDoc(st, docId("file", "/b.rs"));
    expect(st.active).toBe(docId("file", "/a.rs"));
  });

  it("closing the last doc closes the panel", () => {
    let st = openDoc(initialAux, "file", "/a.rs");
    st = closeDoc(st, docId("file", "/a.rs"));
    expect(st.docs).toHaveLength(0);
    expect(st.active).toBe(null);
  });

  it("closing a background doc leaves the focus alone", () => {
    let st = openDoc(initialAux, "file", "/a.rs");
    st = openDoc(st, "file", "/b.rs");
    st = closeDoc(st, docId("file", "/a.rs"));
    expect(st.active).toBe(docId("file", "/b.rs"));
  });
});

describe("auxVisible", () => {
  it("stays hidden with nothing picked and no live agents", () => {
    expect(auxVisible(initialAux, 0)).toBe(false);
  });

  it("auto-opens for a live subagent", () => {
    expect(auxVisible(initialAux, 1)).toBe(true);
  });

  it("stays closed after the reader dismisses the agent cards", () => {
    expect(auxVisible(closeAux(initialAux), 1)).toBe(false);
  });

  it("a picked facet shows regardless of agents", () => {
    expect(auxVisible(toggleFacet(initialAux, "skills"), 0)).toBe(true);
  });

  it("a dismissal is spent once the last subagent finishes", () => {
    const dismissed = closeAux(initialAux);
    const after = syncLiveAgents(dismissed, 0);
    expect(auxVisible(after, 2)).toBe(true);
  });
});

describe("pruneFacet", () => {
  it("drops a facet that no longer has a button", () => {
    const st = toggleFacet(initialAux, "bgtasks");
    expect(pruneFacet(st, () => false).active).toBe(null);
  });

  it("leaves a doc alone", () => {
    const st = openDoc(initialAux, "web", "https://example.com/x");
    expect(pruneFacet(st, () => false).active).toBe(st.active);
  });
});

describe("auxDocLabel", () => {
  it("names a file by its basename, either separator", () => {
    expect(auxDocLabel("file", "/repo/src/main.rs")).toBe("main.rs");
    expect(auxDocLabel("file", "C:\\repo\\main.rs")).toBe("main.rs");
  });

  it("names a wiki doc by its last slug segment", () => {
    expect(auxDocLabel("wiki", "arch/overview")).toBe("overview");
  });

  it("names a page by its host, falling back to the raw string", () => {
    expect(auxDocLabel("web", "https://example.com/deep/path")).toBe("example.com");
    expect(auxDocLabel("web", "not a url")).toBe("not a url");
  });
});
