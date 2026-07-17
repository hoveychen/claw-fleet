import { describe, expect, it } from "vitest";
import { permissionPrimary } from "./permissionPrimary";

describe("permissionPrimary", () => {
  it("Bash leads with the actual command, not the description (security gate)", () => {
    const p = permissionPrimary("Bash", {
      command: "rm -rf /tmp/build",
      description: "Clean the build cache",
    });
    expect(p).toEqual({ kind: "command", text: "rm -rf /tmp/build" });
  });

  it("Read/Edit/Write surface the file path", () => {
    for (const tool of ["Read", "Edit", "Write", "MultiEdit", "NotebookEdit"]) {
      const p = permissionPrimary(tool, { file_path: "/a/b/c.ts", content: "x" });
      expect(p).toEqual({ kind: "file", path: "/a/b/c.ts" });
    }
  });

  it("Grep/Glob surface the pattern and optional path", () => {
    expect(permissionPrimary("Grep", { pattern: "TODO", path: "src" })).toEqual({
      kind: "pattern",
      text: "TODO",
      path: "src",
    });
    expect(permissionPrimary("Glob", { pattern: "**/*.rs" })).toEqual({
      kind: "pattern",
      text: "**/*.rs",
      path: undefined,
    });
  });

  it("WebFetch surfaces the url, WebSearch the query", () => {
    expect(permissionPrimary("WebFetch", { url: "https://x.dev", prompt: "read it" })).toEqual({
      kind: "url",
      text: "https://x.dev",
    });
    expect(permissionPrimary("WebSearch", { query: "rust async" })).toEqual({
      kind: "url",
      text: "rust async",
    });
  });

  it("unknown tools fall back to pretty JSON", () => {
    const p = permissionPrimary("SomeMcpTool", { foo: 1, bar: [2, 3] });
    expect(p.kind).toBe("json");
    expect(p.kind === "json" && p.text).toContain('"foo": 1');
  });

  it("a tool with only non-string inputs falls back to JSON rather than crashing", () => {
    const p = permissionPrimary("Weird", { n: 5, ok: true });
    expect(p.kind).toBe("json");
  });
});
