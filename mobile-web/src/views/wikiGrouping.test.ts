import { describe, expect, it } from "vitest";
import type { WikiDoc } from "../types";
import { groupDocsByRecency } from "./WikiView";

function doc(slug: string, updatedMs: number): WikiDoc {
  return { slug, updatedMs } as unknown as WikiDoc;
}

describe("groupDocsByRecency", () => {
  it("puts the folder with the newest doc first, not the alphabetically first one", () => {
    const docs = [
      doc("arch/new", 300), // 最新
      doc("old-root-doc", 200),
      doc("zzz/mid", 250),
    ].sort((a, b) => b.updatedMs - a.updatedMs);

    expect(groupDocsByRecency(docs).map(([folder]) => folder)).toEqual(["arch", "zzz", ""]);
  });

  it("keeps docs inside a folder in the order they arrive (newest first)", () => {
    const docs = [doc("a/x", 300), doc("a/y", 100), doc("a/z", 50)];
    const [[, items]] = groupDocsByRecency(docs);
    expect(items.map((d) => d.slug)).toEqual(["a/x", "a/y", "a/z"]);
  });

  it("falls back to slug order when two folders share their newest timestamp", () => {
    const docs = [doc("b/one", 500), doc("a/one", 500)];
    expect(groupDocsByRecency(docs).map(([folder]) => folder)).toEqual(["a", "b"]);
  });
});
