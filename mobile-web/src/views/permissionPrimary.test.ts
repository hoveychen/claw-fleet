import { describe, expect, it } from "vitest";
import { permissionPrimary } from "./permissionPrimary";

describe("permissionPrimary (mobile-web)", () => {
  it("Bash leads with the actual command, not the description", () => {
    expect(
      permissionPrimary("Bash", { command: "rm -rf /tmp/x", description: "clean" }),
    ).toEqual({ kind: "command", text: "rm -rf /tmp/x" });
  });

  it("file tools surface the path", () => {
    expect(permissionPrimary("Write", { file_path: "/a/b.ts", content: "x" })).toEqual({
      kind: "file",
      path: "/a/b.ts",
    });
  });

  it("Grep surfaces pattern + path; WebFetch surfaces url", () => {
    expect(permissionPrimary("Grep", { pattern: "TODO", path: "src" })).toEqual({
      kind: "pattern",
      text: "TODO",
      path: "src",
    });
    expect(permissionPrimary("WebFetch", { url: "https://x.dev" })).toEqual({
      kind: "url",
      text: "https://x.dev",
    });
  });

  it("unknown tools fall back to JSON", () => {
    expect(permissionPrimary("McpThing", { a: 1 }).kind).toBe("json");
  });
});
