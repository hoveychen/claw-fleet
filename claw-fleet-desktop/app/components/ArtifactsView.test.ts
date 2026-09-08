import { describe, expect, it } from "vitest";

import {
  DEFAULT_SORT_DIR,
  dropKey,
  dropTargetFolder,
  joinExportPath,
  nextSelection,
  uniqueExportNames,
  buildArtifactDirectoryTree,
  filterArtifacts,
  formatBytes,
  sortArtifacts,
} from "./ArtifactsView";
import type { Artifact } from "./ArtifactsView";

function make(over: Partial<Artifact>): Artifact {
  return {
    id: "20260827-120000",
    name: "a.pdf",
    title: "a.pdf",
    note: "",
    mime: "application/pdf",
    kind: "pdf",
    sizeBytes: 1000,
    createdMs: 1_756_000_000_000,
    workspacePath: "/w/one",
    workspaceName: "one",
    path: "",
    currentVersion: "v1",
    versions: [
      { id: "v1", addedMs: 1_756_000_000_000, sizeBytes: 1000, sourcePath: "/src/a.pdf", hardlinked: true },
    ],
    sessionId: null,
    sourcePath: "/src/a.pdf",
    starred: false,
    hardlinked: true,
    drifted: false,
    ...over,
  };
}

describe("formatBytes", () => {
  it("keeps one decimal only where it carries information", () => {
    expect(formatBytes(512)).toBe("512 B");
    // 1.4 MB must not round to "1 MB" — the difference matters when the number
    // is the only hint of how big a download will be.
    expect(formatBytes(1_468_006)).toBe("1.4 MB");
    // Above 10 the extra digit is noise.
    expect(formatBytes(412_663_296)).toBe("394 MB");
    expect(formatBytes(0)).toBe("0 B");
  });
});

describe("sortArtifacts", () => {
  /**
   * Two `fleet artifact add` calls in one script land in the same millisecond,
   * and a comparator that only looks at createdMs then leaves their order to
   * whatever the directory scan returned — the grid would reshuffle on every
   * refresh. The id's collision suffix is the tiebreak.
   */
  it("puts the newest first and breaks a same-millisecond tie by id", () => {
    const older = make({ id: "20260827-120000", createdMs: 1000 });
    const newer = make({ id: "20260827-130000", createdMs: 2000 });
    const sameMs = make({ id: "20260827-130000-2", createdMs: 2000 });

    const out = sortArtifacts([older, newer, sameMs], "recent");
    expect(out.map((a) => a.id)).toEqual([
      "20260827-130000-2",
      "20260827-130000",
      "20260827-120000",
    ]);
  });

  it("sorts by size descending — the cleanup view's whole point", () => {
    const small = make({ id: "s", sizeBytes: 10 });
    const big = make({ id: "b", sizeBytes: 400_000_000 });
    expect(sortArtifacts([small, big], "size").map((a) => a.id)).toEqual(["b", "s"]);
  });

  it("sorts names with a locale collator, not by code point", () => {
    // A plain `<` comparison orders these by UTF-16 code unit, which scatters
    // CJK titles arbitrarily. localeCompare is what makes the list readable.
    const a = make({ id: "1", title: "报告 2" });
    const b = make({ id: "2", title: "报告 10" });
    const sorted = sortArtifacts([b, a], "name").map((x) => x.title);
    // numeric:true is what puts 2 before 10 rather than "10" before "2".
    expect(sorted).toEqual(["报告 2", "报告 10"]);
  });

  it("does not mutate the input", () => {
    const list = [make({ id: "a", createdMs: 1 }), make({ id: "b", createdMs: 2 })];
    sortArtifacts(list, "recent");
    expect(list.map((x) => x.id)).toEqual(["a", "b"]);
  });
});

describe("filterArtifacts", () => {
  const items = [
    make({ id: "1", title: "Q3 财务分析", note: "给财务的", workspacePath: "/w/one", starred: true }),
    make({ id: "2", title: "launch.mp4", name: "launch.mp4", note: "", workspacePath: "/w/two" }),
    make({ id: "3", title: "评审稿", note: "架构评审会用", workspacePath: "/w/one" }),
  ];
  const all = { query: "", workspace: "", starredOnly: false };

  it("matches the note and the filename, not just the title", () => {
    // What a user remembers is as often "the one about 架构" as the title.
    expect(filterArtifacts(items, { ...all, query: "架构" }).map((a) => a.id)).toEqual(["3"]);
    expect(filterArtifacts(items, { ...all, query: "launch" }).map((a) => a.id)).toEqual(["2"]);
  });

  it("is case-insensitive and ignores surrounding whitespace", () => {
    expect(filterArtifacts(items, { ...all, query: "  LAUNCH  " }).map((a) => a.id)).toEqual(["2"]);
  });

  it("combines workspace and starred filters with the query", () => {
    expect(filterArtifacts(items, { ...all, workspace: "/w/one" }).map((a) => a.id)).toEqual([
      "1",
      "3",
    ]);
    expect(filterArtifacts(items, { ...all, starredOnly: true }).map((a) => a.id)).toEqual(["1"]);
    expect(
      filterArtifacts(items, { query: "财务", workspace: "/w/one", starredOnly: true }).map(
        (a) => a.id,
      ),
    ).toEqual(["1"]);
    // A filter that excludes everything must return nothing, not fall back to all.
    expect(
      filterArtifacts(items, { ...all, workspace: "/w/two", starredOnly: true }),
    ).toHaveLength(0);
  });

  it("returns everything when no filter is set", () => {
    expect(filterArtifacts(items, all)).toHaveLength(3);
  });

  it("filters a selected directory recursively without leaking sibling folders", () => {
    const nested = [
      make({ id: "docs", workspacePath: "/w/one", sourcePath: "/w/one/docs/readme.md" }),
      make({ id: "deep", workspacePath: "/w/one", sourcePath: "/w/one/docs/review/final.pdf" }),
      make({ id: "sibling", workspacePath: "/w/one", sourcePath: "/w/one/src/app.ts" }),
      make({ id: "other", workspacePath: "/w/two", sourcePath: "/w/two/docs/other.md" }),
    ];

    expect(
      filterArtifacts(nested, {
        ...all,
        workspace: "/w/one",
        directory: "docs",
      }).map((artifact) => artifact.id),
    ).toEqual(["docs", "deep"]);
  });
});

describe("buildArtifactDirectoryTree", () => {
  it("builds workspace roots and nested directories with recursive counts", () => {
    const tree = buildArtifactDirectoryTree([
      make({ id: "1", workspacePath: "/w/one", workspaceName: "one", sourcePath: "/w/one/docs/readme.md" }),
      make({ id: "2", workspacePath: "/w/one", workspaceName: "one", sourcePath: "/w/one/docs/review/final.pdf" }),
      make({ id: "3", workspacePath: "/w/one", workspaceName: "one", sourcePath: "/w/one/src/app.ts" }),
      make({ id: "4", workspacePath: "/w/two", workspaceName: "two", sourcePath: "/outside/export.zip" }),
    ]);

    expect(tree.map((node) => [node.label, node.count])).toEqual([
      ["one", 3],
      ["two", 1],
    ]);
    expect(tree[0].children.map((node) => [node.label, node.count])).toEqual([
      ["docs", 2],
      ["src", 1],
    ]);
    expect(tree[0].children[0].children.map((node) => [node.label, node.count])).toEqual([
      ["review", 1],
    ]);
    expect(tree[1].children).toEqual([]);
  });

  it("normalizes Windows separators and keeps deterministic labels", () => {
    const tree = buildArtifactDirectoryTree([
      make({ id: "1", workspacePath: "C:\\repo", workspaceName: "repo", sourcePath: "C:\\repo\\docs\\guide.pdf" }),
    ]);

    expect(tree[0].children.map((node) => node.directory)).toEqual(["docs"]);
  });

  it("files an artifact by its own path, not by where the agent wrote it", () => {
    const tree = buildArtifactDirectoryTree([
      // Written into src/ by the agent, but filed under 交付 by the user.
      make({ id: "1", path: "交付", sourcePath: "/w/one/src/app.ts" }),
      make({ id: "2", path: "交付/2026Q3", sourcePath: "/w/one/src/app.ts" }),
      // Unfiled: still derived from the source path, as before.
      make({ id: "3", path: "", sourcePath: "/w/one/docs/readme.md" }),
    ]);

    expect(tree[0].children.map((node) => [node.label, node.count])).toEqual([
      ["docs", 1],
      ["交付", 2],
    ]);
    expect(tree[0].children[1].children.map((node) => [node.label, node.count])).toEqual([
      ["2026Q3", 1],
    ]);
  });

  it("shows a folder the user made before anything is filed in it", () => {
    const tree = buildArtifactDirectoryTree(
      [make({ id: "1", path: "", sourcePath: "/w/one/a.pdf" })],
      [
        { workspacePath: "/w/one", path: "交付" },
        { workspacePath: "/w/one", path: "交付/2026Q3" },
      ],
    );

    // Present, and counted as empty — an empty folder must not inflate a count.
    expect(tree[0].children.map((node) => [node.label, node.count])).toEqual([["交付", 0]]);
    expect(tree[0].children[0].children.map((node) => node.directory)).toEqual(["交付/2026Q3"]);
  });

  it("names a workspace whose only content is an empty folder", () => {
    const tree = buildArtifactDirectoryTree(
      [make({ id: "1", workspacePath: "/w/one", workspaceName: "one" })],
      [{ workspacePath: "/w/two", path: "空的" }],
    );

    // /w/two has no artifact to carry a display name, so it falls back to the
    // path rather than rendering "undefined".
    expect(tree.map((node) => node.label)).toEqual(["/w/two", "one"]);
  });
});

describe("sortArtifacts direction", () => {
  it("keeps today's order when no direction is given", () => {
    const list = [
      make({ id: "1", sizeBytes: 10 }),
      make({ id: "2", sizeBytes: 3000 }),
    ];
    // The grid never passes a direction, so biggest-first must be unchanged.
    expect(sortArtifacts(list, "size").map((a) => a.id)).toEqual(["2", "1"]);
    expect(sortArtifacts(list, "size", DEFAULT_SORT_DIR.size).map((a) => a.id)).toEqual(["2", "1"]);
  });

  it("reverses a key when asked for its non-default direction", () => {
    const list = [
      make({ id: "1", sizeBytes: 10, createdMs: 100, title: "a" }),
      make({ id: "2", sizeBytes: 3000, createdMs: 900, title: "b" }),
    ];
    expect(sortArtifacts(list, "size", "asc").map((a) => a.id)).toEqual(["1", "2"]);
    expect(sortArtifacts(list, "recent", "asc").map((a) => a.id)).toEqual(["1", "2"]);
    // name defaults to A→Z, so "desc" is the flipped one here — the direction
    // is per key, not one global "descending".
    expect(sortArtifacts(list, "name", "desc").map((a) => a.id)).toEqual(["2", "1"]);
    expect(sortArtifacts(list, "name", "asc").map((a) => a.id)).toEqual(["1", "2"]);
  });

  it("groups by workspace, then folder, for the 来源 column", () => {
    const list = [
      make({ id: "1", workspaceName: "two", path: "a" }),
      make({ id: "2", workspaceName: "one", path: "b" }),
      make({ id: "3", workspaceName: "one", path: "a" }),
    ];
    expect(sortArtifacts(list, "workspace", "asc").map((a) => a.id)).toEqual(["3", "2", "1"]);
    expect(sortArtifacts(list, "workspace", "desc").map((a) => a.id)).toEqual(["1", "2", "3"]);
  });

  it("does not mutate the input", () => {
    const list = [make({ id: "1", sizeBytes: 1 }), make({ id: "2", sizeBytes: 2 })];
    sortArtifacts(list, "size", "asc");
    expect(list.map((a) => a.id)).toEqual(["1", "2"]);
  });
});

describe("nextSelection", () => {
  const order = ["a", "b", "c", "d"];

  it("toggles one id and remembers it as the shift anchor", () => {
    const first = nextSelection(new Set(), order, "b", { shift: false, anchor: null });
    expect([...first.selected]).toEqual(["b"]);
    expect(first.anchor).toBe("b");

    const off = nextSelection(first.selected, order, "b", { shift: false, anchor: first.anchor });
    expect([...off.selected]).toEqual([]);
    // Deselecting the anchor must drop it, or a later shift-click extends from
    // a row that is no longer checked.
    expect(off.anchor).toBeNull();
  });

  it("shift-extends across the displayed order, inclusive of both ends", () => {
    const r = nextSelection(new Set(["b"]), order, "d", { shift: true, anchor: "b" });
    expect([...r.selected].sort()).toEqual(["b", "c", "d"]);
    // The anchor stays put so a second shift-click re-extends from the origin.
    expect(r.anchor).toBe("b");

    const back = nextSelection(r.selected, order, "a", { shift: true, anchor: "b" });
    expect([...back.selected].sort()).toEqual(["a", "b", "c", "d"]);
  });

  it("never deselects on a shift-click", () => {
    const r = nextSelection(new Set(["a", "d"]), order, "b", { shift: true, anchor: "a" });
    expect([...r.selected].sort()).toEqual(["a", "b", "d"]);
  });

  it("falls back to a plain toggle when there is no anchor to extend from", () => {
    const r = nextSelection(new Set(), order, "c", { shift: true, anchor: null });
    expect([...r.selected]).toEqual(["c"]);
    expect(r.anchor).toBe("c");
  });

  it("does not mutate the set it was given", () => {
    const before = new Set(["a"]);
    nextSelection(before, order, "b", { shift: false, anchor: "a" });
    expect([...before]).toEqual(["a"]);
  });
});

describe("uniqueExportNames", () => {
  it("suffixes repeats before the extension so the file still opens", () => {
    expect(uniqueExportNames(["a.pdf", "a.pdf", "a.pdf"])).toEqual([
      "a.pdf",
      "a (2).pdf",
      "a (3).pdf",
    ]);
  });

  it("handles names with no extension and dotfiles", () => {
    expect(uniqueExportNames(["README", "README"])).toEqual(["README", "README (2)"]);
    // A leading dot is the whole name, not an extension — do not turn
    // ".env" into " (2).env".
    expect(uniqueExportNames([".env", ".env"])).toEqual([".env", ".env (2)"]);
  });

  it("does not collide with a name the caller already used", () => {
    expect(uniqueExportNames(["a.pdf", "a (2).pdf", "a.pdf"])).toEqual([
      "a.pdf",
      "a (2).pdf",
      "a (3).pdf",
    ]);
  });

  it("leaves distinct names alone", () => {
    expect(uniqueExportNames(["a.pdf", "b.pdf"])).toEqual(["a.pdf", "b.pdf"]);
  });
});

describe("joinExportPath", () => {
  it("keeps the platform separator the picked directory used", () => {
    expect(joinExportPath("/Users/me/out", "a.pdf")).toBe("/Users/me/out/a.pdf");
    expect(joinExportPath("C:\\Users\\me", "a.pdf")).toBe("C:\\Users\\me\\a.pdf");
  });

  it("does not double the separator on a trailing slash", () => {
    expect(joinExportPath("/out/", "a.pdf")).toBe("/out/a.pdf");
    expect(joinExportPath("C:\\out\\", "a.pdf")).toBe("C:\\out\\a.pdf");
  });
});

describe("dropTargetFolder", () => {
  const inA = [{ workspacePath: "/w/a" }];

  it("reads back the workspace and folder a row encodes", () => {
    expect(dropTargetFolder(dropKey("/w/a", "交付/2026Q3"), inA)).toEqual({
      workspacePath: "/w/a",
      directory: "交付/2026Q3",
    });
    // The workspace row encodes the empty directory — dropping there unfiles.
    expect(dropTargetFolder(dropKey("/w/a", ""), inA)).toEqual({
      workspacePath: "/w/a",
      directory: "",
    });
  });

  it("refuses a drop onto another workspace's folder", () => {
    // "交付" under repo A is not the same place as "交付" under repo B, and
    // silently re-homing a deliverable to a repo it never came from would be
    // the worst possible reading of the gesture.
    expect(dropTargetFolder(dropKey("/w/b", "交付"), inA)).toBeNull();
  });

  it("refuses a mixed selection that spans two workspaces", () => {
    const mixed = [{ workspacePath: "/w/a" }, { workspacePath: "/w/b" }];
    // No single destination means the same thing for both, so nowhere is a
    // legal target — including each of their own folders.
    expect(dropTargetFolder(dropKey("/w/a", "交付"), mixed)).toBeNull();
    expect(dropTargetFolder(dropKey("/w/b", "交付"), mixed)).toBeNull();
  });

  it("is null when the drop landed nowhere, or on nothing", () => {
    expect(dropTargetFolder(null, inA)).toBeNull();
    expect(dropTargetFolder(dropKey("/w/a", "交付"), [])).toBeNull();
    // A key without the separator is not a folder row.
    expect(dropTargetFolder("garbage", inA)).toBeNull();
  });

  it("keeps a folder path containing the separator-free slashes intact", () => {
    expect(dropTargetFolder(dropKey("/w/a", "a/b/c"), inA)?.directory).toBe("a/b/c");
  });
});
