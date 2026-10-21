// The 资料 tree's display nesting (shared-ts/libraryTree.ts). Core decides
// which folder an item is filed in; these pin how that flat answer is drawn.

import { describe, expect, it } from "vitest";
import {
  buildLibraryTree,
  libraryDropKey,
  libraryDropTarget,
  libraryEntryMatches,
  type LibraryEntry,
  type LibraryFolderRec,
} from "../../shared-ts/libraryTree";

const P = "/repo";

function folder(path: string, description = ""): LibraryFolderRec {
  return { workspacePath: P, path, description, wikiPrefix: path, createdMs: 0, updatedMs: 0 };
}

function entry(
  kind: "doc" | "artifact",
  id: string,
  folder: string | null,
  updatedMs = 0,
  title = id,
): LibraryEntry {
  return { kind, id, title, workspacePath: P, folder, location: "", updatedMs, artifactKind: "" };
}

describe("buildLibraryTree", () => {
  it("puts docs and artifacts of one folder side by side and the rest in unfiled", () => {
    const view = buildLibraryTree({
      folders: [folder("reports", "Quarterly"), folder("reports/2026")],
      items: [
        entry("doc", "reports/q3", "reports", 2),
        entry("artifact", "a1", "reports", 3),
        entry("artifact", "a2", "reports/2026", 1),
        entry("doc", "loose", null, 5),
      ],
    });
    expect(view.folders).toHaveLength(1);
    const reports = view.folders[0];
    expect(reports.description).toBe("Quarterly");
    expect(reports.items.map((i) => i.id)).toEqual(["a1", "reports/q3"]);
    expect(reports.total).toBe(3);
    expect(reports.children[0].label).toBe("2026");
    expect(view.unfiled.map((i) => i.id)).toEqual(["loose"]);
    expect(view.shown).toBe(4);
  });

  it("keeps empty folders without a search and prunes them with one", () => {
    const project = {
      folders: [folder("empty"), folder("full")],
      items: [entry("doc", "full/x", "full", 0, "Budget plan")],
    };
    expect(buildLibraryTree(project).folders.map((f) => f.path)).toEqual(["empty", "full"]);
    const searched = buildLibraryTree(project, (e) => libraryEntryMatches(e, "budget"));
    expect(searched.folders.map((f) => f.path)).toEqual(["full"]);
    expect(buildLibraryTree(project, (e) => libraryEntryMatches(e, "nope")).folders).toEqual([]);
  });

  it("treats an item filed in a folder the list lacks as unfiled", () => {
    const view = buildLibraryTree({ folders: [], items: [entry("artifact", "a", "ghost")] });
    expect(view.unfiled).toHaveLength(1);
  });

  it("hangs an orphaned subfolder at the top instead of dropping it", () => {
    const view = buildLibraryTree({ folders: [folder("a/b")], items: [] });
    expect(view.folders.map((f) => f.path)).toEqual(["a/b"]);
  });
});

describe("libraryDropTarget", () => {
  it("accepts a row of the same project, including the unfiled bucket", () => {
    expect(libraryDropTarget(libraryDropKey(P, "reports"), P)).toBe("reports");
    expect(libraryDropTarget(libraryDropKey(P, ""), P)).toBe("");
  });

  it("refuses another project's row and empty space", () => {
    expect(libraryDropTarget(libraryDropKey("/other", "reports"), P)).toBeNull();
    expect(libraryDropTarget(null, P)).toBeNull();
  });
});
