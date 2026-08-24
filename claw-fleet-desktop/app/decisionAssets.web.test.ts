// @vitest-environment jsdom
//
// jsdom rather than the node environment because the browser-build branch
// resolves against `window.location.origin` — same split as
// `userAttachments.web.test.ts` and `wikiAssets.web.test.ts`.
import { describe, expect, it, vi } from "vitest";

/**
 * `hostEnv`'s flag is one-way by design — production code sets it once at boot
 * and never clears it — so switching hosts between cases means a fresh module
 * registry rather than a reset hook that only tests would ever call.
 */
async function loadFor(host: "desktop" | "web") {
  vi.resetModules();
  if (host === "web") (await import("./hostEnv")).markWebBuild();
  return import("./decisionAssets");
}

describe("decisionAssetUrl in the browser build", () => {
  /**
   * `fleet-decision://` is a Tauri custom protocol, registered on the webview
   * and unknown to a plain tab: Chromium blocks the navigation and the card's
   * preview iframe paints an empty rectangle — measured, with the rest of the
   * card (question, options, submit) rendering normally, so nothing on screen
   * says the preview failed.
   *
   * A path, not `/decision_asset?id=`: the served `index.html` reaches the
   * question's images through relative refs (`<img src="chart.png">` is the
   * documented contract for `fleet__ask`'s `images`), and the browser resolves
   * those against the URL's directory.
   */
  it("serves card assets over HTTP instead of the custom protocol", async () => {
    const desktop = await loadFor("desktop");
    expect(desktop.decisionAssetUrl("card-7", "q0")).toBe(
      "fleet-decision://localhost/card-7/q0/index.html",
    );

    const web = await loadFor("web");
    expect(web.decisionAssetUrl("card-7", "q0")).toBe(
      `${window.location.origin}/decision_asset/card-7/q0/index.html`,
    );
  });

  /**
   * Unlike a wiki slug, neither the card id nor `q<idx>` can hold a separator,
   * so nothing needs `%2F` smuggling here — but a `rel` still may be nested,
   * and its slashes have to survive as separators for the relative refs above
   * to land anywhere.
   */
  it("keeps a nested rel's slashes as separators", async () => {
    const web = await loadFor("web");
    expect(web.decisionAssetUrl("card-7", "q2", "sub/chart.png")).toBe(
      `${window.location.origin}/decision_asset/card-7/q2/sub/chart.png`,
    );
  });
});
