import { describe, expect, it } from "vitest";

import {
  filterArtifacts,
  formatBytes,
  sortArtifacts,
  textPreviewMode,
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

describe("textPreviewMode", () => {
  /**
   * The store lumps every `text/*` file into one `text` kind, so before this
   * split a markdown spec rendered as `##` and an html report rendered as its
   * own source. Both are ordinary deliverables — the wiki/artifact routing rule
   * is audience, not file format — so the stage has to tell the three apart.
   */
  it("separates markdown and html from plain text", () => {
    expect(textPreviewMode("text/markdown; charset=utf-8")).toBe("markdown");
    expect(textPreviewMode("text/html; charset=utf-8")).toBe("html");
    expect(textPreviewMode("text/plain; charset=utf-8")).toBe("plain");
    expect(textPreviewMode("text/csv; charset=utf-8")).toBe("plain");
    // application/json is bucketed `text` by the store but is not markup.
    expect(textPreviewMode("application/json")).toBe("plain");
  });

  it("ignores parameter and case noise in the mime", () => {
    // The value comes off the wire; a stricter equality check would silently
    // send a real markdown doc back to the <pre> path.
    expect(textPreviewMode("TEXT/MARKDOWN")).toBe("markdown");
    expect(textPreviewMode(" text/html ")).toBe("html");
    expect(textPreviewMode("")).toBe("plain");
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
});
