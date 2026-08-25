import { describe, expect, it } from "vitest";
import indexHtml from "../index.html?raw";

// 手机打开 `fleet webui` 时把人送到 `/m/`（同源移动端 UI）的那段判定。
//
// 它必须是 index.html 里的**内联**脚本：走 module bundle 的话，手机要先下完
// 整个桌面 bundle 才会跳走，白等一次。而内联脚本天然不可单测。
//
// 所以这里的做法是：脚本把判定挂成一个纯函数，测试从 index.html 里原样抽出这段
// 源码执行，再直接调那个函数。测的就是真正会跑的那份代码 —— 不存在「测试里一
// 份、页面里另一份」的漂移。
type Env = {
  search: string;
  tauri: boolean;
  coarsePointer: boolean;
  minScreenPx: number;
  forcedDesktop: boolean;
  pathname: string;
};

/** 抽出内联脚本并执行，拿到它挂出来的判定函数。 */
function loadDecider(): (env: Env) => string | null {
  const m = indexHtml.match(
    /<script id="mobile-redirect">([\s\S]*?)<\/script>/,
  );
  if (!m) throw new Error('index.html 里找不到 <script id="mobile-redirect">');
  const scope: Record<string, unknown> = {};
  // 脚本在真实页面里会立刻用真环境跑一次；这里给它一个惰性的 window 替身，
  // 让那次自调用无害，只留下挂好的函数。
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

  // 平板：粗指针但屏够大。桌面版在这个尺寸上是能用的，硬塞移动版反而更差。
  it("粗指针但大屏（平板）⇒ 留在桌面版", () => {
    expect(loadDecider()({ ...phone, minScreenPx: 1024 })).toBeNull();
  });

  it("?desktop 强制留在桌面版", () => {
    expect(loadDecider()({ ...phone, search: "?desktop=1" })).toBeNull();
  });

  // 上一次用 ?desktop 选过，之后再打开不带参数也要记住。
  it("记住过的桌面版偏好优先于尺寸判定", () => {
    expect(loadDecider()({ ...phone, forcedDesktop: true })).toBeNull();
  });

  // Tauri 壳加载的是同一份 index.html。带触摸屏的窄窗口不该把桌面 app 自己
  // 跳到一个它根本没有的 /m/ 路径上。
  it("Tauri 壳里绝不重定向", () => {
    expect(loadDecider()({ ...phone, tauri: true })).toBeNull();
  });

  // 跳过去之后 /m/ 由移动端 bundle 接管；万一这段脚本在那儿也跑了（缓存、
  // 误配），不能再跳一次，否则就是死循环。
  it("已经在 /m/ 下不再重定向", () => {
    expect(loadDecider()({ ...phone, pathname: "/m/" })).toBeNull();
  });
});
