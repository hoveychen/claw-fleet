import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import ReactMarkdown from "react-markdown";
import { mdComponents } from "./components";

// 壳身份的两个判据都是可注入的：Capacitor 运行时（mock 掉）和鸿蒙注入的
// `fleetNative` 桥（stub 一个 window）。mobile-web 没装 jsdom，故不碰真 DOM。
const { isNativePlatform } = vi.hoisted(() => ({ isNativePlatform: vi.fn(() => false) }));
vi.mock("@capacitor/core", () => ({ Capacitor: { isNativePlatform } }));

function render(md: string): string {
  return renderToStaticMarkup(<ReactMarkdown components={mdComponents}>{md}</ReactMarkdown>);
}

beforeEach(() => isNativePlatform.mockReturnValue(false));
afterEach(() => vi.unstubAllGlobals());

describe("markdown 链接", () => {
  // 老板在手机上点消息里的论文标题点不开 —— 那一处把 `a` 换成了 `<span>`。
  it("http 链接渲染成真 <a> 且带 href", () => {
    const html = render("见 [论文](https://arxiv.org/abs/1234)");
    expect(html).toContain('href="https://arxiv.org/abs/1234"');
    expect(html).toContain("<a");
  });

  it("mailto 也可点", () => {
    expect(render("[写信](mailto:a@b.c)")).toContain('href="mailto:a@b.c"');
  });

  // 相对路径 / 未知 scheme 在壳里点下去会把整个 SPA 导走，宁可不可点。
  it("相对路径与未知 scheme 不给 href", () => {
    for (const md of ["[本地](./a.md)", "[怪的](fleet-decision://x)"]) {
      expect(render(md)).not.toContain("href=");
    }
  });

  // react-markdown 把 hast 节点当 prop 传进来；spread 到标签上会渲染成
  // node="[object Object]"。
  it("不把 react-markdown 的 node prop 漏成 HTML 属性", () => {
    expect(render("[x](https://e.com) 和 [y](./a.md)")).not.toContain("node=");
  });

  // 壳只认同窗口导航（Capacitor 的 launchIntent / 鸿蒙的 onLoadIntercept），
  // target=_blank 在两边的 WebView 里都被静默丢掉。
  it("浏览器里开新标签，Capacitor 壳里不加 target", () => {
    expect(render("[x](https://e.com)")).toContain('target="_blank"');
    isNativePlatform.mockReturnValue(true);
    expect(render("[x](https://e.com)")).not.toContain("target=");
  });

  it("鸿蒙壳（fleetNative 桥）里同样不加 target", () => {
    vi.stubGlobal("window", { fleetNative: {} });
    expect(render("[x](https://e.com)")).not.toContain("target=");
  });
});
