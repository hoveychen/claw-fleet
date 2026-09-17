import { describe, expect, it } from "vitest";

/**
 * Every `<ReactMarkdown>` must explicitly pass `components`.
 *
 * This guards against a pattern we just fixed: decision/plan tabs, tool details, and Fleet
 * tool results each created a ReactMarkdown but forgot to pass the component map. This caused
 * ```mermaid fences to render as raw code on mobile while showing diagrams on desktop —
 * even though SessionDetailTabs's comments claimed "same as wiki/message view." Forgetting
 * to pass is silent; only manual inspection catches it, so we enforce it here.
 *
 * When adding a new rendering surface: either spread `mermaidMarkdownComponents` (to render
 * diagrams), or explicitly pass a map without it (if you only want plain text). Both cases
 * pass; only forgetting to pass fails.
 */
// Vite's glob import: fetch raw source of every .tsx under src, no need for node:fs
// (mobile-web is a pure browser package; tsconfig has no node types).
const FILES = import.meta.glob("../**/*.tsx", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

describe("markdown rendering surface coverage", () => {
  it("Every <ReactMarkdown> passes components", () => {
    const offenders: string[] = [];
    for (const [path, src] of Object.entries(FILES)) {
      if (path.endsWith(".test.tsx")) continue;
      // Each opening tag up to its `>` is the attribute region.
      for (const m of src.matchAll(/<ReactMarkdown\b[\s\S]*?>/g)) {
        if (!m[0].includes("components=")) {
          offenders.push(`${path}:${src.slice(0, m.index).split("\n").length}`);
        }
      }
    }
    expect(offenders).toEqual([]);
  });

  // Decision cards used to attach only remarkGfm, so the same text rendered correctly
  // in sessions (CJK bold, formulas, soft line breaks all work) but broke entirely in
  // decision cards. Differences between rendering surfaces should only appear in the
  // component map, not in the plugin chain.
  // The message view rewrites `a` as `<span className={styles.mdLink}>`: it looks like
  // a link but clicking does nothing. This has been the case since the first version
  // of session details — only manual testing on mobile surfaces it. The only legal inert
  // link is the band title (it's wrapped in a <button>), explicitly whitelisted.
  const INERT_LINK_OK = new Set(["bandTitleMdComponents"]);
  it("No rendering surface makes links into non-clickable spans", () => {
    const offenders: string[] = [];
    for (const [path, src] of Object.entries(FILES)) {
      if (path.endsWith(".test.tsx")) continue;
      // Catch both patterns: inline in the component map as `a: (…) => <span>`, or
      // declared separately as `const x: Components["a"] = (…) => <span>` then attached
      // (DecisionQa uses the latter, which is why the first version missed it).
      const INERT = /(?:a: |Components\["a"\] = )\(\{[^)]*\}[^)]*\) => \(?\s*<span/g;
      for (const m of src.matchAll(INERT)) {
        const line = src.slice(0, m.index).split("\n").length;
        // Whitelist is identified by "this component map's variable name": search upward for the nearest `const X = {`.
        const decl = [...src.slice(0, m.index).matchAll(/const (\w+)[^=]*= /g)].pop();
        if (decl && INERT_LINK_OK.has(decl[1])) continue;
        offenders.push(`${path}:${line}`);
      }
    }
    expect(offenders).toEqual([]);
  });

  it("Every <ReactMarkdown> uses shared plugin chain", () => {
    const offenders: string[] = [];
    for (const [path, src] of Object.entries(FILES)) {
      if (path.endsWith(".test.tsx")) continue;
      for (const m of src.matchAll(/<ReactMarkdown\b[\s\S]*?>/g)) {
        if (!m[0].includes("remarkPlugins={mdRemarkPlugins}")) {
          offenders.push(`${path}:${src.slice(0, m.index).split("\n").length}`);
        }
      }
    }
    expect(offenders).toEqual([]);
  });
});
