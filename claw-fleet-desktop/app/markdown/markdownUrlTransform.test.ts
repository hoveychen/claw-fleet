// react-markdown filters every URL through `urlTransform`, and its default
// allow-list (http/https/mailto/xmpp/irc) silently blanks a `data:image/…`
// src — no <img>, no failure state, nothing to debug from. `markdownUrlTransform`
// admits image data URLs and nothing else.
//
// The prop is per-render-site, so the second half of this file is a coverage
// check: a new `<ReactMarkdown>` that forgets it reverts to the default for that
// surface only, which is exactly the "fixed one of N surfaces" bug class.
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { markdownUrlTransform } from "./plugins";

describe("markdownUrlTransform", () => {
  it("lets an image data URL through", () => {
    const url = "data:image/png;base64,iVBORw0KGgo=";
    expect(markdownUrlTransform(url)).toBe(url);
    expect(markdownUrlTransform("data:image/svg+xml;utf8,<svg/>")).toContain("svg");
  });

  it("keeps normal links working", () => {
    expect(markdownUrlTransform("https://example.com/a.png")).toBe("https://example.com/a.png");
    expect(markdownUrlTransform("/Users/me/shot.png")).toBe("/Users/me/shot.png");
    expect(markdownUrlTransform("mailto:a@b.c")).toBe("mailto:a@b.c");
  });

  it("still drops the dangerous protocols the default rejects", () => {
    expect(markdownUrlTransform("javascript:alert(1)")).toBe("");
    // Not an image: a data document that could carry markup/script.
    expect(markdownUrlTransform("data:text/html,<script>alert(1)</script>")).toBe("");
  });
});

/** Every `.tsx` under app/, recursively. */
function sourceFiles(dir: string, out: string[] = []): string[] {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) sourceFiles(path, out);
    else if (entry.name.endsWith(".tsx") && !entry.name.includes(".test.")) out.push(path);
  }
  return out;
}

describe("每个 ReactMarkdown 渲染点都传了 urlTransform", () => {
  it("no render site falls back to react-markdown's own allow-list", () => {
    const appDir = join(import.meta.dirname, "..");
    const offenders: string[] = [];

    for (const file of sourceFiles(appDir)) {
      const src = readFileSync(file, "utf8");
      const sites = src.split("<ReactMarkdown").length - 1;
      if (!sites) continue;
      const wired = src.split("urlTransform={markdownUrlTransform}").length - 1;
      if (wired !== sites) offenders.push(`${file}: ${sites} sites, ${wired} wired`);
    }

    expect(offenders, "add urlTransform={markdownUrlTransform} to these").toEqual([]);
  });
});
