// @vitest-environment jsdom
//
// jsdom because the whole point of the module is a browser-build path: it
// resolves URLs against `window.location.origin` and reads fetched bytes with
// `FileReader`.
import { beforeEach, describe, expect, it, vi } from "vitest";

async function loadWeb() {
  vi.resetModules();
  (await import("./hostEnv")).markWebBuild();
  return import("./decisionAssetDoc");
}

const PNG_BYTES = new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10]);

describe("isRelativeAssetRef", () => {
  it("only claims refs that resolve against the asset directory", async () => {
    const { isRelativeAssetRef } = await loadWeb();
    expect(isRelativeAssetRef("chart.png")).toBe(true);
    expect(isRelativeAssetRef("sub/chart.png")).toBe(true);
    // Already reachable or already inline — inlining these would be a wasted
    // fetch at best and a broken rewrite at worst.
    expect(isRelativeAssetRef("data:image/png;base64,AAAA")).toBe(false);
    expect(isRelativeAssetRef("https://example.com/a.png")).toBe(false);
    expect(isRelativeAssetRef("//cdn.example.com/a.png")).toBe(false);
    expect(isRelativeAssetRef("/decision_asset/x/q0/a.png")).toBe(false);
    expect(isRelativeAssetRef("#frag")).toBe(false);
    expect(isRelativeAssetRef("")).toBe(false);
  });
});

describe("inlineRelativeImages", () => {
  it("swaps every relative img src for what the loader returns", async () => {
    const { inlineRelativeImages } = await loadWeb();
    const html =
      '<figure><img src="a.png" alt="x"><img src=\'sub/b.png\'>' +
      '<img src="https://cdn/c.png"></figure>';
    const out = await inlineRelativeImages(html, async (rel) => `data:image/png;base64,${rel}`);
    expect(out).toContain('src="data:image/png;base64,a.png"');
    expect(out).toContain("src='data:image/png;base64,sub/b.png'");
    // Untouched: it was already reachable.
    expect(out).toContain('src="https://cdn/c.png"');
    // Attributes around the src survive the rewrite.
    expect(out).toContain('alt="x"');
  });

  it("leaves a ref whose fetch failed alone rather than blanking the preview", async () => {
    const { inlineRelativeImages } = await loadWeb();
    vi.spyOn(console, "warn").mockImplementation(() => {});
    const html = '<img src="ok.png"><img src="gone.png">';
    const out = await inlineRelativeImages(html, async (rel) => {
      if (rel === "gone.png") throw new Error("HTTP 404");
      return "data:image/png;base64,OK";
    });
    expect(out).toContain('src="data:image/png;base64,OK"');
    expect(out).toContain('src="gone.png"');
  });
});

describe("fetchDecisionAssetDoc", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  /**
   * The bug this module exists for: the sandboxed frame's own image request is
   * cross-site for cookie purposes and a login gateway redirects it. Here the
   * parent does both fetches, so both carry the page's credentials and the
   * frame gets a document with nothing left to request.
   */
  it("inlines the gallery's image and pins the theme", async () => {
    const { fetchDecisionAssetDoc } = await loadWeb();
    const seen: string[] = [];
    const fakeFetch = vi.fn(async (url: string) => {
      seen.push(url);
      if (url.includes("index.html")) {
        return new Response('<figure><img src="teachers-day.png"></figure>', {
          status: 200,
          headers: { "Content-Type": "text/html" },
        });
      }
      return new Response(PNG_BYTES, {
        status: 200,
        headers: { "Content-Type": "image/png" },
      });
    }) as unknown as typeof fetch;

    const doc = await fetchDecisionAssetDoc("card-7", "q0", "dark", fakeFetch);

    expect(seen[0]).toBe(
      `${window.location.origin}/decision_asset/card-7/q0/index.html?theme=dark`,
    );
    expect(seen).toContain(
      `${window.location.origin}/decision_asset/card-7/q0/teachers-day.png`,
    );
    expect(doc).toContain('src="data:image/png;base64,');
    expect(doc).not.toContain('src="teachers-day.png"');
    // `srcDoc` has no URL, so core's `?theme=` prelude cannot fire — the scheme
    // has to be pinned in the document or a dark card renders a white box.
    expect(doc).toContain("color-scheme:dark");
  });

  it("throws when the index itself is unreachable, so the caller can fall back", async () => {
    const { fetchDecisionAssetDoc } = await loadWeb();
    const fakeFetch = vi.fn(
      async () => new Response("nope", { status: 502 }),
    ) as unknown as typeof fetch;
    await expect(fetchDecisionAssetDoc("card-7", "q0", "dark", fakeFetch)).rejects.toThrow(
      /502/,
    );
  });

  /**
   * A gateway that answers 200 with a login page instead of the bytes would
   * otherwise be inlined as an `image/…`-shaped data URL that silently fails to
   * decode. Keep the original ref instead — the frame's own request may still
   * work on an ungated deployment.
   */
  it("refuses an image request that answered HTML", async () => {
    const { fetchDecisionAssetDoc } = await loadWeb();
    vi.spyOn(console, "warn").mockImplementation(() => {});
    const fakeFetch = vi.fn(async (url: string) =>
      url.includes("index.html")
        ? new Response('<img src="a.png">', {
            status: 200,
            headers: { "Content-Type": "text/html" },
          })
        : new Response("<!doctype html><title>Sign in</title>", {
            status: 200,
            headers: { "Content-Type": "text/html; charset=utf-8" },
          }),
    ) as unknown as typeof fetch;

    const doc = await fetchDecisionAssetDoc("card-7", "q0", undefined, fakeFetch);
    expect(doc).toContain('src="a.png"');
    expect(doc).not.toContain("data:text/html");
  });
});
