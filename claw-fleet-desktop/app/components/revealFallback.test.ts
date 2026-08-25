import { describe, expect, it } from "vitest";
import { planRevealFallback } from "./revealFallback";

describe("planRevealFallback", () => {
  // The bug that motivated all of this: a decision card said
  // `public/app-icon.png`, meaning it relative to claw-fleet-desktop/. The chip
  // joined it onto the workspace root, the tree found nothing, and said nothing.
  it("retries at the real path when the agent named it relative to a subdirectory", () => {
    expect(
      planRevealFallback("public/app-icon.png", ["claw-fleet-desktop/public/app-icon.png"]),
    ).toEqual({ kind: "retry", relPath: "claw-fleet-desktop/public/app-icon.png" });
  });

  it("says missing when no file in the workspace fits", () => {
    expect(planRevealFallback("public/app-icon.png", [])).toEqual({ kind: "missing" });
  });

  // The path was right all along — the tree just filters it out (gitignored,
  // 「显示忽略文件」 off). Retrying it would fail exactly the same way.
  it("previews out-of-tree when the tried path is itself a hit", () => {
    expect(planRevealFallback("dist/bundle.js", ["dist/bundle.js"])).toEqual({
      kind: "preview",
      relPath: "dist/bundle.js",
    });
  });

  // Termination: a retry that also fails comes back here with the retried path
  // as `triedRel`, now matching itself — so it lands on preview, not another retry.
  it("cannot loop: the second failure of a retried path becomes a preview", () => {
    const first = planRevealFallback("icon.png", ["app/icon.png"]);
    expect(first).toEqual({ kind: "retry", relPath: "app/icon.png" });
    expect(planRevealFallback("app/icon.png", ["app/icon.png"])).toEqual({
      kind: "preview",
      relPath: "app/icon.png",
    });
  });

  it("refuses to guess between several matches", () => {
    expect(planRevealFallback("public/icon.png", ["a/public/icon.png", "b/public/icon.png"])).toEqual(
      { kind: "ambiguous", candidates: ["a/public/icon.png", "b/public/icon.png"] },
    );
  });

  it("normalises separators and stray slashes before comparing", () => {
    // A Windows-flavoured tried path must still recognise itself in the hits,
    // and duplicates collapse rather than faking ambiguity.
    expect(planRevealFallback("dist\\bundle.js", ["/dist/bundle.js/", "dist/bundle.js"])).toEqual({
      kind: "preview",
      relPath: "dist/bundle.js",
    });
  });
});
