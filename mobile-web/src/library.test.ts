import { describe, expect, it } from "vitest";
import { filterLibrary, libraryProjects, mergeLibrary, taskLibrary } from "./library";
import type { Artifact, WikiDoc } from "./types";

function doc(over: Partial<WikiDoc>): WikiDoc {
  return {
    slug: "design/x",
    title: "X",
    kind: "markdown",
    entry: "index.md",
    workspacePath: "/work/fleet",
    workspaceName: "fleet",
    createdMs: 1,
    updatedMs: 1,
    currentVersion: "v1",
    versions: [],
    ...over,
  };
}

function art(over: Partial<Artifact>): Artifact {
  return {
    id: "a",
    name: "out.pdf",
    title: "Report",
    note: "",
    mime: "application/pdf",
    kind: "pdf",
    sizeBytes: 10,
    createdMs: 1,
    workspacePath: "/work/fleet",
    workspaceName: "fleet",
    path: "",
    sessionId: null,
    sourcePath: "",
    starred: false,
    hardlinked: false,
    drifted: false,
    currentVersion: "v1",
    versions: [],
    ...over,
  };
}

describe("mergeLibrary", () => {
  it("interleaves docs and artifacts newest first", () => {
    const items = mergeLibrary(
      [doc({ slug: "old", updatedMs: 5 }), doc({ slug: "new", updatedMs: 30 })],
      [art({ id: "mid", createdMs: 20 })],
    );
    expect(items.map((i) => i.key)).toEqual(["doc:new", "artifact:mid", "doc:old"]);
  });

  it("falls back to the slug leaf and file name for missing titles", () => {
    const items = mergeLibrary([doc({ slug: "a/b/leaf", title: "" })], [art({ title: "", name: "f.zip" })]);
    expect(items.map((i) => i.title).sort()).toEqual(["f.zip", "leaf"]);
  });
});

describe("filterLibrary", () => {
  const items = mergeLibrary(
    [
      doc({ slug: "arch", title: "Architecture", workspacePath: "/work/fleet" }),
      doc({ slug: "notes", title: "Notes", workspacePath: "/work/other", workspaceName: "other" }),
    ],
    [art({ id: "r", title: "Report", note: "quarterly numbers" })],
  );

  it("filters by type", () => {
    expect(filterLibrary(items, { type: "artifact" }).map((i) => i.key)).toEqual(["artifact:r"]);
  });

  it("filters by project, matching a worktree to its repo", () => {
    const wt = mergeLibrary([doc({ slug: "wt", workspacePath: "/work/fleet/.worktrees/p" })], []);
    expect(filterLibrary(wt, { project: "/work/fleet" })).toHaveLength(1);
    expect(filterLibrary(items, { project: "/work/other" }).map((i) => i.key)).toEqual(["doc:notes"]);
  });

  it("searches titles and artifact notes, plus full-text doc hits", () => {
    expect(filterLibrary(items, { query: "quarterly" }).map((i) => i.key)).toEqual(["artifact:r"]);
    expect(
      filterLibrary(items, { query: "zzz", extraDocSlugs: new Set(["notes"]) }).map((i) => i.key),
    ).toEqual(["doc:notes"]);
  });

  it("lists distinct projects by name", () => {
    expect(libraryProjects(items).map((p) => p.name)).toEqual(["fleet", "other"]);
  });
});

describe("taskLibrary", () => {
  it("takes the task's artifacts and its project's docs", () => {
    const items = mergeLibrary(
      [doc({ slug: "mine" }), doc({ slug: "theirs", workspacePath: "/work/other" })],
      [art({ id: "ours", sessionId: "s1" }), art({ id: "stray", sessionId: "s9" }), art({ id: "none" })],
    );
    const { outputs, docs } = taskLibrary(items, new Set(["s1", "s2"]), "/work/fleet/.worktrees/x");
    expect(outputs.map((i) => i.key)).toEqual(["artifact:ours"]);
    expect(docs.map((i) => i.key)).toEqual(["doc:mine"]);
  });
});
