import { describe, expect, it } from "vitest";
import { normalizeSlug, slugBasename } from "./wikiSlug";

/**
 * These expectations are lifted from core's `normalize_slug_basics` and
 * `normalize_slug_keeps_directory_segments` tests. They are the contract: if
 * the two implementations disagree, the publish dialog mistakes an existing
 * doc for a new one and a "replace" silently discards it.
 */
describe("normalizeSlug mirrors core", () => {
  it("folds case and non-alphanumeric runs", () => {
    expect(normalizeSlug("My Report V2")).toBe("my-report-v2");
    expect(normalizeSlug("foo__bar--baz")).toBe("foo-bar-baz");
    expect(normalizeSlug("-lead-trail-")).toBe("lead-trail");
    // CJK is not slug material — core drops it the same way.
    expect(normalizeSlug("中文 report")).toBe("report");
  });

  it("returns empty when nothing survives", () => {
    expect(normalizeSlug("")).toBe("");
    expect(normalizeSlug("汉字")).toBe("");
    expect(normalizeSlug("///")).toBe("");
    expect(normalizeSlug("../..")).toBe("");
  });

  it("keeps directory segments and drops empty ones", () => {
    expect(normalizeSlug("arch/overview")).toBe("arch/overview");
    expect(normalizeSlug("Arch/Storage Layer")).toBe("arch/storage-layer");
    expect(normalizeSlug("/arch//overview/")).toBe("arch/overview");
    // Path traversal is unrepresentable: the dots normalize away.
    expect(normalizeSlug("../../etc/passwd")).toBe("etc/passwd");
  });
});

describe("slugBasename", () => {
  it("returns the last segment", () => {
    expect(slugBasename("arch/deep/overview")).toBe("overview");
    expect(slugBasename("flat")).toBe("flat");
  });
});
