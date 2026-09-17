import { describe, expect, it } from "vitest";
import indexHtml from "../index.html?raw";

// Logic that redirects mobile users opening `fleet webui` to `/m/` (same-origin
// mobile UI).
//
// Must be an **inline** script in index.html: if it went through the module
// bundle, the phone would download the entire desktop bundle first, then
// redirect—wasted time. Inline scripts normally can't be unit tested.
//
// Solution: the script exposes the redirect decision as a pure function. The
// test extracts the script source from index.html and executes it, then calls
// the function directly. This tests the actual code that runs—no drift between
// test and production versions.
type Env = {
  search: string;
  tauri: boolean;
  coarsePointer: boolean;
  minScreenPx: number;
  forcedDesktop: boolean;
  pathname: string;
};

/** Extract the inline script, execute it, and return the exposed decision
 *  function. */
function loadDecider(): (env: Env) => string | null {
  const m = indexHtml.match(
    /<script id="mobile-redirect">([\s\S]*?)<\/script>/,
  );
  if (!m) throw new Error('index.html 里找不到 <script id="mobile-redirect">');
  const scope: Record<string, unknown> = {};
  // The script runs immediately with the real environment on the real page;
  // give it a lazy window stub here so the self-call is harmless and only the
  // exposed function remains.
  const fakeWindow = {
    __fleetDecideMobileRedirect: undefined as unknown,
    location: { search: "", pathname: "/", hash: "", replace: () => {} },
    localStorage: { getItem: () => null, setItem: () => {} },
    matchMedia: () => ({ matches: false }),
    screen: { width: 1920, height: 1080 },
  };
  scope.window = fakeWindow;
  new Function("window", indexHtml && m[1])(fakeWindow);
  const fn = fakeWindow.__fleetDecideMobileRedirect;
  if (typeof fn !== "function") throw new Error("脚本没有挂出判定函数");
  return fn as (env: Env) => string | null;
}

const phone: Env = {
  search: "",
  tauri: false,
  coarsePointer: true,
  minScreenPx: 390,
  forcedDesktop: false,
  pathname: "/",
};

describe("手机访问 fleet webui 的重定向判定", () => {
  it("粗指针 + 窄屏 ⇒ 去 /m/", () => {
    expect(loadDecider()(phone)).toBe("/m/");
  });

  it("查询串一并带过去，免得深链在跳转里丢掉", () => {
    expect(loadDecider()({ ...phone, search: "?a=1&b=2" })).toBe("/m/?a=1&b=2");
  });

  it("鼠标 + 大屏 ⇒ 留在桌面版", () => {
    expect(
      loadDecider()({ ...phone, coarsePointer: false, minScreenPx: 1440 }),
    ).toBeNull();
  });

  // Tablet: coarse pointer but large screen. Desktop version works fine at this
  // size; forcing mobile would be worse.
  it("粗指针但大屏（平板）⇒ 留在桌面版", () => {
    expect(loadDecider()({ ...phone, minScreenPx: 1024 })).toBeNull();
  });

  it("?desktop 强制留在桌面版", () => {
    expect(loadDecider()({ ...phone, search: "?desktop=1" })).toBeNull();
  });

  // User chose ?desktop once; remember that preference on later visits without
  // the param.
  it("记住过的桌面版偏好优先于尺寸判定", () => {
    expect(loadDecider()({ ...phone, forcedDesktop: true })).toBeNull();
  });

  // The Tauri shell loads the same index.html. A narrow touch-capable window
  // shouldn't redirect the desktop app itself to /m/, a path that doesn't exist.
  it("Tauri 壳里绝不重定向", () => {
    expect(loadDecider()({ ...phone, tauri: true })).toBeNull();
  });

  // After redirect, /m/ is handled by the mobile bundle. If this script somehow
  // runs there too (cache, misconfiguration), it must not redirect again or
  // we'd loop forever.
  it("已经在 /m/ 下不再重定向", () => {
    expect(loadDecider()({ ...phone, pathname: "/m/" })).toBeNull();
  });
});
