// @vitest-environment jsdom
//
// jsdom rather than the node environment because the browser-build branch
// resolves against `window.location.origin` — same split as
// `wikiAssets.web.test.ts`.
import { describe, expect, it, vi } from "vitest";

/** `hostEnv`'s flag is one-way by design, so switching hosts between cases
 *  means a fresh module registry rather than a test-only reset hook. */
async function loadFor(host: "desktop" | "web") {
  vi.resetModules();
  if (host === "web") (await import("./hostEnv")).markWebBuild();
  return import("./artifactAssets");
}

describe("artifactBlobUrl", () => {
  /**
   * `fleet-artifact://` is a Tauri custom protocol, registered on the webview
   * and unknown to a plain tab — Chromium refuses the navigation and the
   * element paints nothing, with no visible error. The browser build gets the
   * same bytes from `/artifact_blob`, which honours `Range` exactly like the
   * protocol handler, so a `<video>` can still seek in a tab.
   */
  it("uses the custom protocol on desktop and HTTP in the browser build", async () => {
    const desktop = await loadFor("desktop");
    expect(desktop.artifactBlobUrl("20260827-150239", "render.mp4")).toBe(
      "fleet-artifact://localhost/20260827-150239/render.mp4",
    );

    const web = await loadFor("web");
    expect(web.artifactBlobUrl("20260827-150239", "render.mp4")).toBe(
      `${window.location.origin}/artifact_blob?id=20260827-150239`,
    );
  });

  /**
   * The name is cosmetic for routing — the host splits on the first `/` and
   * uses only the id — but it carries the extension the viewer sniffs to pick
   * a decoder, so it must survive spaces and CJK intact rather than being
   * dropped or breaking the URL into extra segments.
   */
  it("encodes a name with spaces and CJK without adding segments", async () => {
    const desktop = await loadFor("desktop");
    const url = desktop.artifactBlobUrl("20260827-150239", "Q3 财务分析.xlsx");
    expect(url).toBe(
      "fleet-artifact://localhost/20260827-150239/Q3%20%E8%B4%A2%E5%8A%A1%E5%88%86%E6%9E%90.xlsx",
    );
    // One separator only: id / name. Anything more and the host's split-on-
    // first-slash would still work, but the name would arrive truncated.
    expect(url.replace("fleet-artifact://localhost/", "").split("/")).toHaveLength(2);
  });

  /** An id with nothing exotic in it must not get mangled either. */
  it("round-trips the id through decodeURIComponent", async () => {
    const desktop = await loadFor("desktop");
    const id = "20260827-150239-2";
    const url = desktop.artifactBlobUrl(id, "a.pdf");
    const firstSegment = url.replace("fleet-artifact://localhost/", "").split("/")[0];
    expect(decodeURIComponent(firstSegment)).toBe(id);
  });
});
