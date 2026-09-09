import { describe, expect, it } from "vitest";
import {
  auxDocLabel,
  closeAux,
  closeDoc,
  collapseDoc,
  docId,
  initialAux,
  makeAuxDoc,
  MAX_AUX_DOCS,
  openDoc,
  pruneTab,
  showFacet,
  toggleDoc,
  type AuxState,
} from "./detailAux";

describe("showFacet", () => {
  it("opens a facet in the drawer", () => {
    expect(showFacet(initialAux, "tokens").active).toBe("tokens");
  });

  it("switching facets keeps the drawer open", () => {
    const open = showFacet(initialAux, "tokens");
    expect(showFacet(open, "skills").active).toBe("skills");
  });

  it("never touches the rail", () => {
    const st = showFacet(openDoc(initialAux, "file", "/a.rs"), "tokens");
    expect(st.docs).toHaveLength(1);
    expect(st.expanded).toBe(docId("file", "/a.rs"));
  });
});

describe("toggleDoc", () => {
  it("expands a carded doc", () => {
    const st = collapseDoc(openDoc(initialAux, "file", "/a.rs"));
    expect(toggleDoc(st, docId("file", "/a.rs")).expanded).toBe(docId("file", "/a.rs"));
  });

  it("clicking the expanded card collapses it, card and all", () => {
    const st = openDoc(initialAux, "file", "/a.rs");
    const collapsed = toggleDoc(st, docId("file", "/a.rs"));
    expect(collapsed.expanded).toBe(null);
    expect(collapsed.docs).toHaveLength(1);
  });

  it("ignores an id with no card", () => {
    expect(toggleDoc(initialAux, docId("file", "/gone.rs")).expanded).toBe(null);
  });

  // The drawer is facet-only now: a doc must never be able to reach `active`,
  // which is what put the same name on screen twice.
  it("leaves the drawer closed", () => {
    const st = openDoc(initialAux, "file", "/a.rs");
    expect(st.active).toBe(null);
    expect(toggleDoc(st, docId("file", "/a.rs")).active).toBe(null);
  });
});

describe("openDoc", () => {
  it("adds the rail card and expands it", () => {
    const st = openDoc(initialAux, "file", "/repo/src/main.rs");
    expect(st.docs).toHaveLength(1);
    expect(st.expanded).toBe(docId("file", "/repo/src/main.rs"));
    expect(st.docs[0].label).toBe("main.rs");
  });

  it("reveals rather than duplicates an already-carded doc", () => {
    let st = openDoc(initialAux, "wiki", "arch/overview");
    st = collapseDoc(st);
    st = openDoc(st, "wiki", "arch/overview");
    expect(st.docs).toHaveLength(1);
    expect(st.expanded).toBe(docId("wiki", "arch/overview"));
  });

  it("trims the oldest card past the cap and keeps the newest expanded", () => {
    let st: AuxState = initialAux;
    for (let i = 0; i < MAX_AUX_DOCS + 2; i += 1) {
      st = openDoc(st, "file", `/repo/f${i}.rs`);
    }
    expect(st.docs).toHaveLength(MAX_AUX_DOCS);
    expect(st.docs[0].ref).toBe("/repo/f2.rs");
    expect(st.expanded).toBe(docId("file", `/repo/f${MAX_AUX_DOCS + 1}.rs`));
  });

  it("a doc id can never collide with a facet name", () => {
    const st = openDoc(initialAux, "file", "tokens");
    expect(st.expanded).not.toBe("tokens");
  });
});

describe("closeAux", () => {
  it("closes the drawer but keeps the rail's cards", () => {
    const st = closeAux(showFacet(openDoc(initialAux, "file", "/a.rs"), "tokens"));
    expect(st.active).toBe(null);
    expect(st.docs).toHaveLength(1);
    expect(st.expanded).toBe(docId("file", "/a.rs"));
  });
});

describe("closeDoc", () => {
  it("dismissing the expanded card collapses rather than sliding to a neighbour", () => {
    let st = openDoc(initialAux, "file", "/a.rs");
    st = openDoc(st, "file", "/b.rs");
    st = closeDoc(st, docId("file", "/b.rs"));
    expect(st.docs).toHaveLength(1);
    expect(st.expanded).toBe(null);
  });

  it("dismissing the last card leaves nothing expanded", () => {
    let st = openDoc(initialAux, "file", "/a.rs");
    st = closeDoc(st, docId("file", "/a.rs"));
    expect(st.docs).toHaveLength(0);
    expect(st.expanded).toBe(null);
  });

  it("dismissing some other card leaves the reader alone", () => {
    let st = openDoc(initialAux, "file", "/a.rs");
    st = openDoc(st, "file", "/b.rs");
    st = closeDoc(st, docId("file", "/a.rs"));
    expect(st.expanded).toBe(docId("file", "/b.rs"));
  });
});

describe("pruneTab", () => {
  it("drops a facet the session no longer offers", () => {
    const st = showFacet(initialAux, "bgtasks");
    expect(pruneTab(st, () => false).active).toBe(null);
  });

  it("keeps a facet that still exists", () => {
    const st = showFacet(initialAux, "tokens");
    expect(pruneTab(st, () => true).active).toBe("tokens");
  });

  it("never prunes the expanded doc — the rail owns that", () => {
    const st = openDoc(initialAux, "web", "https://example.com/x");
    expect(pruneTab(st, () => false).expanded).toBe(st.expanded);
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

describe("artifact docs", () => {
  it("labels an artifact card with the title the caller passes, not its id", () => {
    // The store id ("20260909-080326") names nothing to a reader, so the ingest
    // card hands the deliverable's title down with it.
    const st = openDoc(initialAux, "artifact", "20260909-080326", "9/8 对外更新日志");
    expect(st.docs).toHaveLength(1);
    expect(st.docs[0]).toMatchObject({
      id: "artifact:20260909-080326",
      kind: "artifact",
      ref: "20260909-080326",
      label: "9/8 对外更新日志",
    });
    // …and that id is what `expanded` holds, which is how the transcript card
    // knows its second click should navigate instead of re-opening.
    expect(st.expanded).toBe("artifact:20260909-080326");
  });

  it("falls back to the id when no label is given", () => {
    expect(makeAuxDoc("artifact", "20260909-080326").label).toBe("20260909-080326");
  });
});
