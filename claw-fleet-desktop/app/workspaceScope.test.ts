import { describe, expect, it } from "vitest";
import {
  filterByWorkspaceScope,
  parseWorkspaceScope,
  withWorkspaceScope,
} from "../../shared-ts/workspaceScope";

describe("parseWorkspaceScope", () => {
  it("returns the query untouched when there is no token", () => {
    expect(parseWorkspaceScope("  fix login ")).toEqual({ workspace: null, text: "fix login" });
  });

  it("splits a leading token from the remaining text", () => {
    expect(parseWorkspaceScope("ws:human-zoo y13 breeds")).toEqual({
      workspace: "human-zoo",
      text: "y13 breeds",
    });
  });

  it("finds a token in the middle and is case-insensitive on the prefix", () => {
    expect(parseWorkspaceScope("breeds WS:human-zoo y13")).toEqual({
      workspace: "human-zoo",
      text: "breeds y13",
    });
  });

  it("reads a quoted name with spaces", () => {
    expect(parseWorkspaceScope('ws:"My Repo" x')).toEqual({ workspace: "My Repo", text: "x" });
  });

  it("drops a bare token without scoping", () => {
    expect(parseWorkspaceScope("ws: foo")).toEqual({ workspace: null, text: "foo" });
  });

  it("does not treat a mid-word ws: as a token", () => {
    expect(parseWorkspaceScope("news:today")).toEqual({ workspace: null, text: "news:today" });
  });

  it("lets the last non-empty token win", () => {
    expect(parseWorkspaceScope("ws:a ws:b").workspace).toBe("b");
  });
});

describe("withWorkspaceScope", () => {
  it("prepends a token and keeps the user's text", () => {
    expect(withWorkspaceScope("y13", "human-zoo")).toBe("ws:human-zoo y13");
  });

  it("replaces an existing token", () => {
    expect(withWorkspaceScope("ws:purple y13", "human-zoo")).toBe("ws:human-zoo y13");
  });

  it("leaves a trailing space after a lone token so typing continues the query", () => {
    expect(withWorkspaceScope("", "human-zoo")).toBe("ws:human-zoo ");
  });

  it("quotes names with spaces and round-trips them", () => {
    const q = withWorkspaceScope("", "My Repo");
    expect(q).toBe('ws:"My Repo" ');
    expect(parseWorkspaceScope(q).workspace).toBe("My Repo");
  });

  it("strips the scope when name is null", () => {
    expect(withWorkspaceScope("ws:human-zoo y13", null)).toBe("y13");
  });
});

describe("filterByWorkspaceScope", () => {
  const rows = [
    { id: 1, workspaceName: "claude-fleet" },
    { id: 2, workspaceName: "claude-fleet-cloud" },
    { id: 3, workspaceName: "human-zoo" },
  ];
  const ids = (xs: { id: number }[]) => xs.map((x) => x.id);

  it("passes everything through when unscoped", () => {
    expect(ids(filterByWorkspaceScope(rows, null))).toEqual([1, 2, 3]);
  });

  it("prefers an exact name over substring matches", () => {
    expect(ids(filterByWorkspaceScope(rows, "Claude-Fleet"))).toEqual([1]);
  });

  it("falls back to substring when no name matches exactly", () => {
    expect(ids(filterByWorkspaceScope(rows, "zoo"))).toEqual([3]);
    expect(ids(filterByWorkspaceScope(rows, "claude"))).toEqual([1, 2]);
  });
});
