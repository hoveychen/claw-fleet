// @vitest-environment jsdom
//
// jsdom rather than the node environment because the browser-build branch
// resolves against `window.location.origin` — the same reason
// `userAttachments.web.test.ts` is split out from its node-environment sibling.
import { describe, expect, it, vi } from "vitest";

/**
 * `hostEnv`'s flag is one-way by design — production code sets it once at boot
 * and never clears it — so switching hosts between cases means a fresh module
 * registry rather than a reset hook that only tests would ever call.
 */
async function loadFor(host: "desktop" | "web") {
  vi.resetModules();
  if (host === "web") (await import("./hostEnv")).markWebBuild();
  return import("./wikiAssets");
}

describe("wikiFileUrl in the browser build", () => {
  /**
   * `fleet-wiki://` is a Tauri custom protocol, registered on the webview and
   * unknown to a plain tab: Chromium refuses the navigation outright
   * ("Navigation to external protocol blocked by sandbox") and the iframe
   * paints nothing — measured, with no visible error anywhere in the UI.
   *
   * The replacement has to stay a *path*, not the existing `/wiki_file?slug=`
   * query route: a published `index.html` reaches its bundle through relative
   * refs, and those resolve against the URL's directory. Collapsing the doc
   * into one query-bearing segment sends `assets/style.css` to the wrong place.
   */
  it("serves wiki files over HTTP instead of the custom protocol", async () => {
    const desktop = await loadFor("desktop");
    expect(desktop.wikiFileUrl("arch/overview", "20260824-115141", "index.html")).toBe(
      "fleet-wiki://localhost/arch%2Foverview/20260824-115141/index.html",
    );

    const web = await loadFor("web");
    expect(web.wikiFileUrl("arch/overview", "20260824-115141", "index.html")).toBe(
      `${window.location.origin}/wiki_asset/arch%2Foverview/20260824-115141/index.html`,
    );
  });

  /**
   * The slug's own `/` is the one separator that must NOT survive as a segment
   * boundary — the server splits the tail into exactly three parts
   * (slug / version / rel), so a two-level slug would otherwise eat the
   * version. `%2F` keeps it one opaque segment; browsers preserve it verbatim,
   * which is what lets the relative refs below still land in the right dir.
   */
  it("keeps a multi-segment slug in one segment while leaving rel's slashes alone", async () => {
    const web = await loadFor("web");
    expect(web.wikiFileUrl("a/b/c", "v1", "assets/deep/app.js")).toBe(
      `${window.location.origin}/wiki_asset/a%2Fb%2Fc/v1/assets/deep/app.js`,
    );
  });
});
