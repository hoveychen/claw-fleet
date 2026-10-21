import { describe, expect, it } from "vitest";

import {
  defaultLibraryProject,
  filterListing,
  findLibraryFolder,
  folderCards,
  folderListing,
  foldersAwaitingReview,
  libraryTypeGroup,
  orderLibraryProjects,
  typeCounts,
} from "../../shared-ts/libraryBrowse";
import { buildLibraryTree, type LibraryEntry, type LibraryProjectTree } from "../../shared-ts/libraryTree";

const WS = "/w/p";

function entry(id: string, folder: string | null, updatedMs: number, artifactKind = ""): LibraryEntry {
  return {
    kind: artifactKind ? "artifact" : "doc",
    id,
    title: id,
    workspacePath: WS,
    folder,
    location: "",
    updatedMs,
    artifactKind,
  };
}

function project(items: LibraryEntry[], ws = WS): LibraryProjectTree {
  const folder = (path: string, origin: LibraryProjectTree["folders"][number]["origin"] = null) => ({
    workspacePath: ws,
    path,
    description: `${path} desc`,
    wikiPrefix: path,
    createdMs: 0,
    updatedMs: 0,
    origin,
  });
  return {
    workspacePath: ws,
    workspaceName: ws.slice(ws.lastIndexOf("/") + 1),
    folders: [
      folder("a"),
      folder("a/x", { source: "upload", reason: "r", createdMs: 1, acceptedMs: null }),
      folder("b", { source: "agent", reason: "r", createdMs: 1, acceptedMs: 5 }),
    ],
    items,
  };
}

describe("libraryBrowse", () => {
  const p = project([
    entry("a1", "a", 10),
    entry("ax1", "a/x", 30, "image"),
    entry("ax2", "a/x", 20, "pdf"),
    entry("b1", "b", 40, "video"),
    entry("u1", null, 50, "archive"),
  ]);
  const view = buildLibraryTree(p);

  it("groups wiki docs with document-like artifacts", () => {
    expect(libraryTypeGroup({ kind: "doc", artifactKind: "" })).toBe("doc");
    expect(libraryTypeGroup({ kind: "artifact", artifactKind: "pdf" })).toBe("doc");
    expect(libraryTypeGroup({ kind: "artifact", artifactKind: "image" })).toBe("image");
    expect(libraryTypeGroup({ kind: "artifact", artifactKind: "archive" })).toBe("other");
  });

  it("lists a folder with everything beneath it, labelled by subfolder", () => {
    const a = findLibraryFolder(view.folders, "a")!;
    const rows = folderListing(a);
    expect(rows.map((r) => [r.entry.id, r.sub])).toEqual([
      ["a1", ""],
      ["ax1", "x"],
      ["ax2", "x"],
    ]);
    expect(findLibraryFolder(view.folders, "a/x")?.label).toBe("x");
    expect(findLibraryFolder(view.folders, "missing")).toBeNull();
  });

  it("filters by type and orders by time either way", () => {
    const rows = folderListing(findLibraryFolder(view.folders, "a")!);
    expect(filterListing(rows, null, "newest").map((r) => r.entry.id)).toEqual(["ax1", "ax2", "a1"]);
    expect(filterListing(rows, null, "oldest").map((r) => r.entry.id)).toEqual(["a1", "ax2", "ax1"]);
    expect(filterListing(rows, "doc", "newest").map((r) => r.entry.id)).toEqual(["ax2", "a1"]);
    expect(typeCounts(rows)).toEqual({ doc: 2, image: 1 });
  });

  it("builds overview cards with the newest items beneath each folder", () => {
    const cards = folderCards(view, 2);
    expect(cards.map((c) => [c.node.path, c.recent.map((e) => e.id)])).toEqual([
      ["a", ["ax1", "ax2"]],
      ["b", ["b1"]],
    ]);
  });

  it("collects only AI folders still awaiting review, at any depth", () => {
    expect(foldersAwaitingReview(view.folders).map((f) => f.path)).toEqual(["a/x"]);
  });

  it("opens on the last project, else the most recently active one", () => {
    const quiet = project([entry("q", null, 1)], "/w/quiet");
    const empty = { ...project([], "/w/empty"), folders: [] };
    expect(defaultLibraryProject([quiet, p, empty], "/w/empty")).toBe("/w/empty");
    expect(defaultLibraryProject([quiet, p, empty], "/w/gone")).toBe(WS);
    expect(defaultLibraryProject([], null)).toBe("");
    expect(orderLibraryProjects([empty, quiet, p]).map((x) => x.workspacePath)).toEqual([
      WS,
      "/w/quiet",
      "/w/empty",
    ]);
  });
});
