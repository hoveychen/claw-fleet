import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import ReactMarkdown from "react-markdown";
import { mdComponents } from "./components";

// Both shell identity checks are injectable: Capacitor runtime (mock) and Harmony-injected
// `fleetNative` bridge (stub a window). mobile-web lacks jsdom, so we don't touch real DOM.
const { isNativePlatform } = vi.hoisted(() => ({ isNativePlatform: vi.fn(() => false) }));
vi.mock("@capacitor/core", () => ({ Capacitor: { isNativePlatform } }));

function render(md: string): string {
  return renderToStaticMarkup(<ReactMarkdown components={mdComponents}>{md}</ReactMarkdown>);
}

beforeEach(() => isNativePlatform.mockReturnValue(false));
afterEach(() => vi.unstubAllGlobals());

describe("markdown 链接", () => {
  // User couldn't click paper titles in messages on phone — that spot swapped `a` for `<span>`.
  it("http links render as real <a> with href", () => {
    const html = render("见 [论文](https://arxiv.org/abs/1234)");
    expect(html).toContain('href="https://arxiv.org/abs/1234"');
    expect(html).toContain("<a");
  });

  it("mailto is also clickable", () => {
    expect(render("[写信](mailto:a@b.c)")).toContain('href="mailto:a@b.c"');
  });

  // Relative paths / unknown schemes navigate away from the whole SPA in shell, better not clickable.
  it("relative paths and unknown schemes get no href", () => {
    for (const md of ["[本地](./a.md)", "[怪的](fleet-decision://x)"]) {
      expect(render(md)).not.toContain("href=");
    }
  });

  // react-markdown passes hast nodes as props; spreading to a tag renders as
  // node="[object Object]".
  it("don't leak react-markdown's node prop as HTML attribute", () => {
    expect(render("[x](https://e.com) 和 [y](./a.md)")).not.toContain("node=");
  });

  // Shell only recognizes window navigation (Capacitor's launchIntent / Harmony's onLoadIntercept),
  // target=_blank is silently dropped in WebView on both sides.
  it("opens new tab in browser, Capacitor shell doesn't add target", () => {
    expect(render("[x](https://e.com)")).toContain('target="_blank"');
    isNativePlatform.mockReturnValue(true);
    expect(render("[x](https://e.com)")).not.toContain("target=");
  });

  it("Harmony shell (fleetNative bridge) likewise doesn't add target", () => {
    vi.stubGlobal("window", { fleetNative: {} });
    expect(render("[x](https://e.com)")).not.toContain("target=");
  });
});
