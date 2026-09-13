import { describe, expect, it } from "vitest";
import type { WikiDoc } from "../types";
import { sortDocsByRecency } from "./WikiView";

function doc(slug: string, updatedMs: number): WikiDoc {
  return { slug, updatedMs } as unknown as WikiDoc;
}

describe("sortDocsByRecency", () => {
  it("is a flat newest-first list — a root doc no longer outranks a newer foldered one", () => {
    const docs = [doc("old-root-doc", 200), doc("arch/new", 300), doc("zzz/mid", 250)];
    expect(sortDocsByRecency(docs).map((d) => d.slug)).toEqual([
      "arch/new",
      "zzz/mid",
      "old-root-doc",
    ]);
  });

  it("falls back to slug order when two docs share a timestamp", () => {
    const docs = [doc("b/one", 500), doc("a/one", 500)];
    expect(sortDocsByRecency(docs).map((d) => d.slug)).toEqual(["a/one", "b/one"]);
  });

  it("does not mutate the input array", () => {
    const docs = [doc("a", 1), doc("b", 2)];
    sortDocsByRecency(docs);
    expect(docs.map((d) => d.slug)).toEqual(["a", "b"]);
  });
});
