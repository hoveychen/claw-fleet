import { describe, expect, it } from "vitest";
import {
  auxDocLabel,
  closeAux,
  closeDoc,
  docId,
  initialAux,
  MAX_AUX_DOCS,
  openDoc,
  pruneTab,
  showTab,
  toggleTab,
  type AuxState,
} from "./detailAux";

describe("toggleTab", () => {
  it("opens a facet in the drawer", () => {
    expect(toggleTab(initialAux, "tokens").active).toBe("tokens");
  });

  it("clicking the showing facet closes the drawer", () => {
    const open = toggleTab(initialAux, "tokens");
    expect(toggleTab(open, "tokens").active).toBe(null);
  });

  it("switching facets keeps the drawer open", () => {
    const open = toggleTab(initialAux, "tokens");
    expect(toggleTab(open, "skills").active).toBe("skills");
  });
});

describe("openDoc", () => {
  it("adds the rail card and opens the doc in the drawer", () => {
    const st = openDoc(initialAux, "file", "/repo/src/main.rs");
    expect(st.docs).toHaveLength(1);
    expect(st.active).toBe(docId("file", "/repo/src/main.rs"));
    expect(st.docs[0].label).toBe("main.rs");
  });

  it("reveals rather than duplicates an already-carded doc", () => {
    let st = openDoc(initialAux, "wiki", "arch/overview");
    st = toggleTab(st, "tokens");
    st = openDoc(st, "wiki", "arch/overview");
    expect(st.docs).toHaveLength(1);
    expect(st.active).toBe(docId("wiki", "arch/overview"));
  });

  it("trims the oldest card past the cap and keeps the newest open", () => {
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

describe("closeAux", () => {
  it("closes the drawer but keeps the rail's cards", () => {
    const st = closeAux(openDoc(initialAux, "file", "/a.rs"));
    expect(st.active).toBe(null);
    expect(st.docs).toHaveLength(1);
  });
});

describe("closeDoc", () => {
  it("dismissing the open doc's card closes the drawer rather than sliding to a neighbour", () => {
    let st = openDoc(initialAux, "file", "/a.rs");
    st = openDoc(st, "file", "/b.rs");
    st = closeDoc(st, docId("file", "/b.rs"));
    expect(st.docs).toHaveLength(1);
    expect(st.active).toBe(null);
  });

  it("dismissing the last card closes the drawer", () => {
    let st = openDoc(initialAux, "file", "/a.rs");
    st = closeDoc(st, docId("file", "/a.rs"));
    expect(st.docs).toHaveLength(0);
    expect(st.active).toBe(null);
  });

  it("dismissing some other card leaves the drawer alone", () => {
    let st = openDoc(initialAux, "file", "/a.rs");
    st = openDoc(st, "file", "/b.rs");
    st = closeDoc(st, docId("file", "/a.rs"));
    expect(st.active).toBe(docId("file", "/b.rs"));
  });
});

describe("pruneTab", () => {
  it("drops a facet the session no longer offers", () => {
    const st = toggleTab(initialAux, "bgtasks");
    expect(pruneTab(st, () => false).active).toBe(null);
  });

  it("keeps content that still exists", () => {
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
