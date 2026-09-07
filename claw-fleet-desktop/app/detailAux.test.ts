import { describe, expect, it } from "vitest";
import {
  activeAuxTab,
  AGENTS_TAB,
  auxDocLabel,
  closeAux,
  closeDoc,
  docId,
  initialAux,
  MAX_AUX_DOCS,
  openDoc,
  pruneTab,
  showTab,
  syncLiveAgents,
  toggleTab,
  type AuxState,
} from "./detailAux";

describe("toggleTab", () => {
  it("opens a facet", () => {
    expect(toggleTab(initialAux, "tokens").active).toBe("tokens");
  });

  it("clicking the showing facet closes the panel", () => {
    const open = toggleTab(initialAux, "tokens");
    expect(toggleTab(open, "tokens").active).toBe(null);
  });

  it("switching facets keeps the panel open", () => {
    const open = toggleTab(initialAux, "tokens");
    expect(toggleTab(open, "skills").active).toBe("skills");
  });

  it("clears a previous dismissal so the panel actually reopens", () => {
    const dismissed = closeAux(initialAux);
    expect(dismissed.agentsDismissed).toBe(true);
    expect(toggleTab(dismissed, "skills").agentsDismissed).toBe(false);
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
    st = toggleTab(st, "tokens");
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

describe("activeAuxTab", () => {
  it("is closed with nothing picked and no live agents", () => {
    expect(activeAuxTab(initialAux, 0)).toBe(null);
  });

  it("auto-opens on the agent deck for a live subagent", () => {
    expect(activeAuxTab(initialAux, 1)).toBe(AGENTS_TAB);
  });

  it("stays closed after the reader dismisses the deck", () => {
    expect(activeAuxTab(closeAux(initialAux), 1)).toBe(null);
  });

  it("a picked facet wins over the auto-open", () => {
    expect(activeAuxTab(toggleTab(initialAux, "skills"), 3)).toBe("skills");
  });

  it("a dismissal is spent once the last subagent finishes", () => {
    const dismissed = closeAux(initialAux);
    const after = syncLiveAgents(dismissed, 0);
    expect(activeAuxTab(after, 2)).toBe(AGENTS_TAB);
  });

  it("showTab reopens a panel the reader had closed", () => {
    const dismissed = closeAux(initialAux);
    expect(activeAuxTab(showTab(dismissed, "tokens"), 0)).toBe("tokens");
  });
});

describe("pruneTab", () => {
  it("drops a facet whose tab is gone", () => {
    const st = toggleTab(initialAux, "bgtasks");
    expect(pruneTab(st, () => false).active).toBe(null);
  });

  it("drops the agent deck once the last subagent finishes", () => {
    const st = toggleTab(initialAux, AGENTS_TAB);
    expect(pruneTab(st, (id) => id !== AGENTS_TAB).active).toBe(null);
  });

  it("keeps a tab that still exists", () => {
    const st = openDoc(initialAux, "web", "https://example.com/x");
    expect(pruneTab(st, () => true).active).toBe(st.active);
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
