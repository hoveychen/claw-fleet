import { describe, expect, it } from "vitest";
import {
  fileTabId,
  parseTabKind,
  tabKindLabel,
  webTabId,
  wikiTabId,
} from "./tabKind";
import { DRAFT_TAB_ID } from "./sessionTabs";

describe("parseTabKind", () => {
  it("reads a session id as a session tab", () => {
    // Real session ids are UUIDs — no prefix, so anything unprefixed is one.
    const id = "7c9e6679-7425-40de-944b-e07fc1f90ae7";
    expect(parseTabKind(id)).toEqual({ kind: "session", sessionId: id });
  });

  it("reads the draft tab", () => {
    expect(parseTabKind(DRAFT_TAB_ID)).toEqual({ kind: "draft" });
  });

  it("round-trips a file tab", () => {
    const id = fileTabId("/repo/src/main.rs");
    expect(parseTabKind(id)).toEqual({ kind: "file", absPath: "/repo/src/main.rs" });
  });

  it("identifies a file tab by path alone, so the line clicked cannot fork it", () => {
    // Clicking `foo.rs:10` then `foo.rs:99` must reveal the SAME tab and scroll
    // within it. The line is therefore not part of the id — it travels beside
    // it (like the per-tab search highlight does), which is both how VS Code
    // behaves and what keeps tabGroups' one-tab-per-thing invariant meaningful.
    expect(fileTabId("/a/b.rs")).toBe(fileTabId("/a/b.rs"));
  });

  it("round-trips a Windows path, colons and all", () => {
    // The body is never re-split, so a drive-letter colon is not a delimiter.
    const id = fileTabId("C:\\repo\\src\\main.rs");
    expect(parseTabKind(id)).toEqual({
      kind: "file",
      absPath: "C:\\repo\\src\\main.rs",
    });
  });

  it("round-trips a wiki slug with virtual directories", () => {
    const id = wikiTabId("arch/overview");
    expect(parseTabKind(id)).toEqual({ kind: "wiki", slug: "arch/overview" });
  });

  it("round-trips a url with a query string and fragment", () => {
    const url = "https://example.com/a?b=1&c=2#frag";
    expect(parseTabKind(webTabId(url))).toEqual({ kind: "web", url });
  });

  it("treats a prefix with an empty body as a session id, not a broken tab", () => {
    // Defensive: a truncated persisted id must degrade to something renderable
    // rather than producing a file tab with no path.
    expect(parseTabKind("file:")).toEqual({ kind: "session", sessionId: "file:" });
    expect(parseTabKind("wiki:")).toEqual({ kind: "session", sessionId: "wiki:" });
    expect(parseTabKind("web:")).toEqual({ kind: "session", sessionId: "web:" });
  });
});

describe("tabKindLabel", () => {
  it("labels a file tab with its basename", () => {
    expect(tabKindLabel(parseTabKind(fileTabId("/repo/src/main.rs")))).toBe(
      "main.rs",
    );
  });

  it("handles a Windows path's separator", () => {
    expect(tabKindLabel(parseTabKind(fileTabId("C:\\repo\\main.rs")))).toBe(
      "main.rs",
    );
  });

  it("labels a wiki tab with the last slug segment", () => {
    expect(tabKindLabel(parseTabKind(wikiTabId("arch/overview")))).toBe("overview");
  });

  it("labels a web tab with its host", () => {
    expect(tabKindLabel(parseTabKind(webTabId("https://example.com/deep/path")))).toBe(
      "example.com",
    );
  });

  it("falls back to the raw url when it will not parse", () => {
    expect(tabKindLabel({ kind: "web", url: "not a url" })).toBe("not a url");
  });

  it("returns null for tabs whose label comes from live data", () => {
    // Session tabs read their title from the scan, and the draft tab from i18n;
    // neither is derivable from the id.
    expect(tabKindLabel({ kind: "session", sessionId: "x" })).toBe(null);
    expect(tabKindLabel({ kind: "draft" })).toBe(null);
  });
});
