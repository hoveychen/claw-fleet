import { describe, expect, it } from "vitest";
import { parsePathRef, resolvePathRef } from "./pathRef";

describe("parsePathRef — accepts", () => {
  it("absolute posix paths", () => {
    expect(parsePathRef("/Users/hoveychen/foo.png")).toEqual({
      path: "/Users/hoveychen/foo.png",
      line: null,
    });
  });

  it("home-relative paths", () => {
    expect(parsePathRef("~/.claude/settings.json")).toEqual({
      path: "~/.claude/settings.json",
      line: null,
    });
  });

  it("repo-relative paths with an extension", () => {
    expect(parsePathRef("claw-fleet-core/src/backend.rs")).toEqual({
      path: "claw-fleet-core/src/backend.rs",
      line: null,
    });
  });

  it("explicitly-relative paths", () => {
    expect(parsePathRef("./app/store.ts")).toEqual({ path: "./app/store.ts", line: null });
    expect(parsePathRef("../sibling/mod.rs")).toEqual({ path: "../sibling/mod.rs", line: null });
  });

  it("a trailing slash marks a directory", () => {
    expect(parsePathRef("app/markdown/")).toEqual({ path: "app/markdown/", line: null });
  });

  it("file:line — the clickable form CLAUDE.md tells agents to emit", () => {
    expect(parsePathRef("src/backend.rs:42")).toEqual({ path: "src/backend.rs", line: 42 });
  });

  it("file:line:column keeps the line, drops the column", () => {
    expect(parsePathRef("app/components/FilesView.tsx:120:5")).toEqual({
      path: "app/components/FilesView.tsx",
      line: 120,
    });
  });

  it("CJK path segments", () => {
    expect(parsePathRef("docs/设计/概览.md")).toEqual({ path: "docs/设计/概览.md", line: null });
  });
});

describe("parsePathRef — Windows paths", () => {
  it("absolute drive paths with backslashes", () => {
    expect(parsePathRef("C:\\code\\proj\\src\\main.rs")).toEqual({
      path: "C:\\code\\proj\\src\\main.rs",
      line: null,
    });
  });

  it("absolute drive paths with forward slashes", () => {
    expect(parsePathRef("C:/code/proj/src/main.rs")).toEqual({
      path: "C:/code/proj/src/main.rs",
      line: null,
    });
  });

  it("drive path with :line keeps the drive colon and strips the line", () => {
    expect(parsePathRef("C:\\code\\proj\\src\\main.rs:42")).toEqual({
      path: "C:\\code\\proj\\src\\main.rs",
      line: 42,
    });
  });

  it("backslash-relative path with an extension", () => {
    expect(parsePathRef("src\\backend.rs")).toEqual({ path: "src\\backend.rs", line: null });
  });

  it("still rejects a stray non-drive colon", () => {
    expect(parsePathRef("C:\\code\\a:b.rs")).toBeNull();
    expect(parsePathRef("key:value")).toBeNull();
  });
});

describe("parsePathRef — rejects", () => {
  // Each of these appears in agent prose inside backticks. A false positive here
  // turns ordinary technical writing into a field of blue links.
  const rejected: Array<[string, string]> = [
    ["useState", "bare identifier, no slash"],
    ["foo.rs", "bare filename — no directory, can't be resolved to one file"],
    ["npm run build", "shell command (whitespace)"],
    ["@tauri-apps/plugin-opener", "npm scoped package"],
    ["std::fs", "Rust module path"],
    ["https://example.com/a.html", "URL — handled by the external-link path"],
    ["file:///Users/foo", "URL scheme"],
    ["and/or", "prose with a slash, no extension, no path prefix"],
    ["TypeScript/JavaScript", "prose with a slash"],
    ["1/2", "a fraction"],
    ["a > b", "code fragment"],
    ["foo(bar)", "a call expression"],
    ["{ path: string }", "a type literal"],
    ["", "empty"],
  ];
  it.each(rejected)("rejects %j (%s)", (input) => {
    expect(parsePathRef(input)).toBeNull();
  });
});

describe("resolvePathRef", () => {
  const ws = "/Users/hoveychen/workspace/claude-fleet";

  it("passes absolute paths through", () => {
    expect(resolvePathRef("/etc/hosts", ws, "/Users/hoveychen")).toBe("/etc/hosts");
  });

  it("expands ~ against the home dir", () => {
    expect(resolvePathRef("~/.claude/settings.json", ws, "/Users/hoveychen")).toBe(
      "/Users/hoveychen/.claude/settings.json",
    );
  });

  it("joins repo-relative paths onto the workspace root", () => {
    expect(resolvePathRef("src/backend.rs", ws, "/Users/hoveychen")).toBe(`${ws}/src/backend.rs`);
  });

  it("normalises ./ and ../ segments", () => {
    expect(resolvePathRef("./app/store.ts", ws, "/Users/hoveychen")).toBe(`${ws}/app/store.ts`);
    expect(resolvePathRef("app/../src/main.rs", ws, "/Users/hoveychen")).toBe(`${ws}/src/main.rs`);
  });

  it("cannot expand ~ when the home dir is unknown", () => {
    expect(resolvePathRef("~/foo", ws, null)).toBeNull();
  });

  it("refuses to escape above the filesystem root", () => {
    expect(resolvePathRef("../../../../../../etc/passwd", "/a/b", "/home")).toBe("/etc/passwd");
  });

  describe("Windows", () => {
    const winWs = "C:\\Users\\hovey\\ws";

    it("passes drive-absolute paths through, normalised to forward slashes", () => {
      expect(resolvePathRef("C:\\code\\a.rs", winWs, null)).toBe("C:/code/a.rs");
      expect(resolvePathRef("D:/data/x.csv", winWs, null)).toBe("D:/data/x.csv");
    });

    it("joins relative paths onto a backslash workspace root", () => {
      expect(resolvePathRef("src/backend.rs", winWs, null)).toBe(
        "C:/Users/hovey/ws/src/backend.rs",
      );
    });

    it("normalises ../ against a drive root without escaping the drive", () => {
      expect(resolvePathRef("..\\..\\..\\..\\etc", "C:\\a\\b", null)).toBe("C:/etc");
    });

    it("keeps a trailing separator on directory refs", () => {
      expect(resolvePathRef("C:\\code\\dir\\", winWs, null)).toBe("C:/code/dir/");
    });
  });
});
