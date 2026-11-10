// @vitest-environment jsdom
//
// jsdom for `FileReader`, which turns fetched bytes into `data:` URLs.
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  fetchSandboxDoc,
  inlineDocAssets,
  isRelativeAssetRef,
  resolveAssetPath,
} from "./sandboxDoc";

const PNG_BYTES = new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10]);

beforeEach(() => {
  vi.restoreAllMocks();
});

/** A loader over an in-memory file tree, recording what was asked for. */
function treeLoader(files: Record<string, [string | Uint8Array, string]>) {
  const asked: string[] = [];
  const load = async (path: string) => {
    asked.push(path);
    const hit = files[path];
    if (!hit) throw new Error(`HTTP 404 ${path}`);
    return new Blob([hit[0]], { type: hit[1] });
  };
  return { load, asked };
}

function decodeDataUrl(url: string): string {
  const comma = url.indexOf(",");
  const meta = url.slice(0, comma);
  const body = url.slice(comma + 1);
  return meta.endsWith(";base64") ? atob(body) : decodeURIComponent(body);
}

describe("isRelativeAssetRef", () => {
  it("only claims refs that resolve against the document directory", () => {
    expect(isRelativeAssetRef("chart.png")).toBe(true);
    expect(isRelativeAssetRef("sub/chart.png")).toBe(true);
    expect(isRelativeAssetRef("../up.png")).toBe(true);
    // Already reachable or already inline.
    expect(isRelativeAssetRef("data:image/png;base64,AAAA")).toBe(false);
    expect(isRelativeAssetRef("https://example.com/a.png")).toBe(false);
    expect(isRelativeAssetRef("//cdn.example.com/a.png")).toBe(false);
    expect(isRelativeAssetRef("/decision_asset/x/q0/a.png")).toBe(false);
    expect(isRelativeAssetRef("#frag")).toBe(false);
    expect(isRelativeAssetRef("")).toBe(false);
  });
});

describe("resolveAssetPath", () => {
  it("resolves against the base file's directory", () => {
    expect(resolveAssetPath("index.html", "assets/app.js")).toBe("assets/app.js");
    expect(resolveAssetPath("index.html", "./assets/app.js")).toBe("assets/app.js");
    expect(resolveAssetPath("assets/style.css", "../img/a.png")).toBe("img/a.png");
    expect(resolveAssetPath("assets/style.css", "font.woff2")).toBe("assets/font.woff2");
  });

  it("drops query and fragment and decodes escapes", () => {
    expect(resolveAssetPath("index.html", "my%20pic.png?v=2#x")).toBe("my pic.png");
  });

  it("refuses to climb above the root", () => {
    expect(resolveAssetPath("index.html", "../secret")).toBeNull();
    expect(resolveAssetPath("index.html", "?only-query")).toBeNull();
  });
});

describe("inlineDocAssets", () => {
  it("swaps relative img srcs, quoted or not, and keeps other attributes", async () => {
    const { load } = treeLoader({
      "a.png": [PNG_BYTES, "image/png"],
      "sub/b.png": [PNG_BYTES, "image/png"],
      "d.png": [PNG_BYTES, "image/png"],
    });
    const html =
      '<img src="a.png" alt="x"><img src=\'sub/b.png\'><img src=d.png>' +
      '<img src="https://cdn/c.png"><img data-src="a.png">';
    const out = await inlineDocAssets(html, "index.html", load);
    expect(out).toContain('<img src="data:image/png;base64,iVBORw0KGgo=" alt="x">');
    expect(out).toContain("<img src='data:image/png;base64,");
    // Unquoted comes back double-quoted.
    expect(out).toContain('<img src="data:image/png;base64,iVBORw0KGgo=">');
    expect(out).toContain('src="https://cdn/c.png"');
    // `data-src` is not `src`.
    expect(out).toContain('<img data-src="a.png">');
  });

  /**
   * The shape a multi-file wiki HTML doc has: `index.html` pulling a script
   * and a stylesheet from `assets/`, the stylesheet pulling an image relative
   * to itself.
   */
  it("inlines scripts, stylesheets and the url() refs inside them", async () => {
    const { load, asked } = treeLoader({
      "assets/app.js": ['document.body.dataset.ok = "1";', "application/octet-stream"],
      "assets/style.css": ['body{background:url("../img/bg.png")} h1{color:red}', "text/css"],
      "img/bg.png": [PNG_BYTES, "image/png"],
      "img/logo.png": [PNG_BYTES, "image/png"],
    });
    const html = [
      '<link rel="stylesheet" href="assets/style.css">',
      '<link rel="preconnect" href="assets/style.css">',
      "<style>.logo{background:url(img/logo.png)}</style>",
      '<script type="module" src="./assets/app.js" defer></script>',
    ].join("\n");
    const out = await inlineDocAssets(html, "index.html", load);

    const script = out.match(/<script type="module" src="([^"]+)" defer>/);
    expect(script).not.toBeNull();
    // Forced to a JS MIME whatever the server said — a module script with the
    // wrong MIME refuses to run.
    expect(script![1].startsWith("data:text/javascript;base64,")).toBe(true);
    expect(decodeDataUrl(script![1])).toBe('document.body.dataset.ok = "1";');

    const sheet = out.match(/<link rel="stylesheet" href="([^"]+)">/);
    expect(sheet![1].startsWith("data:text/css;charset=utf-8,")).toBe(true);
    const css = decodeDataUrl(sheet![1]);
    expect(css).toContain('url("data:image/png;base64,iVBORw0KGgo=")');
    expect(css).toContain("h1{color:red}");

    // Only stylesheet / icon links are fetched.
    expect(out).toContain('<link rel="preconnect" href="assets/style.css">');
    expect(out).toContain('.logo{background:url("data:image/png;base64,');
    // Fetched once each even when referenced twice.
    expect(asked.filter((p) => p === "assets/style.css")).toHaveLength(1);
  });

  it("leaves a ref whose fetch failed alone rather than blanking the preview", async () => {
    vi.spyOn(console, "warn").mockImplementation(() => {});
    const { load } = treeLoader({ "ok.png": [PNG_BYTES, "image/png"] });
    const out = await inlineDocAssets('<img src="ok.png"><img src="gone.png">', "index.html", load);
    expect(out).toContain('src="data:image/png;base64,');
    expect(out).toContain('src="gone.png"');
  });

  /**
   * A gateway that answers 200 with a login page instead of the bytes would
   * otherwise be inlined as an image or script that silently fails.
   */
  it("refuses an asset that answered HTML", async () => {
    vi.spyOn(console, "warn").mockImplementation(() => {});
    const { load } = treeLoader({ "a.js": ["<!doctype html><title>Sign in</title>", "text/html"] });
    const out = await inlineDocAssets('<script src="a.js"></script>', "index.html", load);
    expect(out).toBe('<script src="a.js"></script>');
  });
});

describe("fetchSandboxDoc", () => {
  it("fetches the entry and its assets through urlFor", async () => {
    const seen: string[] = [];
    const fakeFetch = vi.fn(async (url: string) => {
      seen.push(url);
      if (url.endsWith("/docs/index.html")) {
        return new Response('<img src="pic.png">', { headers: { "Content-Type": "text/html" } });
      }
      return new Response(PNG_BYTES, { headers: { "Content-Type": "image/png" } });
    }) as unknown as typeof fetch;
    const doc = await fetchSandboxDoc(
      "docs/index.html",
      (p) => `https://h/wiki/${p}`,
      fakeFetch,
    );
    expect(seen).toEqual(["https://h/wiki/docs/index.html", "https://h/wiki/docs/pic.png"]);
    expect(doc).toContain('src="data:image/png;base64,');
  });

  it("leaves refs alone where the store has no URL for them", async () => {
    vi.spyOn(console, "warn").mockImplementation(() => {});
    const fakeFetch = vi.fn(
      async () => new Response('<img src="pic.png">', { headers: { "Content-Type": "text/html" } }),
    ) as unknown as typeof fetch;
    const doc = await fetchSandboxDoc(
      "index.html",
      (p) => (p === "index.html" ? "https://h/artifact_blob?id=1" : null),
      fakeFetch,
    );
    expect(doc).toBe('<img src="pic.png">');
    expect(fakeFetch).toHaveBeenCalledTimes(1);
  });

  it("throws when the entry is unreachable", async () => {
    const fakeFetch = vi.fn(async () => new Response("nope", { status: 502 })) as unknown as typeof fetch;
    await expect(fetchSandboxDoc("index.html", () => "https://h/x", fakeFetch)).rejects.toThrow(/502/);
  });
});
