// @vitest-environment jsdom
//
// 主机给的界面语言默认值（后端的 FLEET_LOCALE，经 host_features 送来）。
//
// 和精简模式的主机默认值同构，存在的理由也一样：浏览器构建的设置各存一份
// localStorage，所以一台配了 FLEET_LOCALE=zh 的主机没有任何办法告诉它服务的
// 页面「我是中文主机」——每个访客都是英文，直到自己翻到设置里那个开关。
//
// 单独一个文件而不是并进 store.test.ts：这些用例要真的加载 i18n，而 i18n 在
// 初始化时读 `navigator.language`，所以必须跑在 jsdom 里（store.test.ts 是
// node 环境）。四条不变量：
//
//   1. 主机表了态、这个客户端没选过 ⇒ 就地换语言，不必等下一次加载；
//   2. 用户显式选过 ⇒ 主机不许改回来；
//   3. 主机的答案缓存下来给下一次同步读（i18next 的 lng 必须同步定），主机
//      不再表态时这份缓存跟着清掉，不会变成撤不掉的粘滞开关；
//   4. 主机报了个这个 bundle 没有的语言 ⇒ 记下来但不生效。
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => undefined) }));
vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => {}),
}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ setTheme: vi.fn(async () => undefined) }),
}));

describe("界面语言的主机默认值（host_features）", () => {
  beforeEach(() => vi.resetModules());

  it("adopts the host's language in place and caches it for the next load", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    vi.mocked(invoke).mockResolvedValueOnce({ terminal: false, localeDefault: "zh" });

    // 先加载 i18n：它这时读到的是空缓存，所以落在浏览器语言（jsdom 报 en-US）
    // 上 —— 这样下面断言的才是「就地换」，而不是「初始化时正好读到了」。
    const i18n = (await import("./i18n")).default;
    expect(i18n.language).toBe("en");

    const { useUIStore } = await import("./store");
    const { getItem } = await import("./storage");
    await useUIStore.getState().loadHostFeatures();

    expect(i18n.language).toBe("zh");
    // 缓存，不是用户的选择：后者必须仍然是「没表过态」。
    expect(getItem("lang-host-default")).toBe("zh");
    expect(getItem("lang")).toBe(null);
  });

  it("boots straight into the cached host language", async () => {
    const { setItem } = await import("./storage");
    setItem("lang-host-default", "zh");

    const i18n = (await import("./i18n")).default;

    expect(i18n.language).toBe("zh");
  });

  it("lets an explicit choice beat the host default", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    vi.mocked(invoke).mockResolvedValueOnce({ terminal: false, localeDefault: "zh" });

    const { setItem, getItem } = await import("./storage");
    setItem("lang", "en");

    const i18n = (await import("./i18n")).default;
    const { useUIStore } = await import("./store");
    await useUIStore.getState().loadHostFeatures();

    expect(i18n.language).toBe("en");
    // 缓存仍然记下主机的意见 —— 用户日后清掉自己的选择时才接得上。
    expect(getItem("lang-host-default")).toBe("zh");
  });

  it("drops the cache when the host stops having an opinion", async () => {
    const { setItem, getItem } = await import("./storage");
    setItem("lang-host-default", "zh");

    const { invoke } = await import("@tauri-apps/api/core");
    vi.mocked(invoke).mockResolvedValueOnce({ terminal: false });

    const { useUIStore } = await import("./store");
    await useUIStore.getState().loadHostFeatures();

    expect(getItem("lang-host-default")).toBe(null);
  });

  it("records a language it has no bundle for without switching to it", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    vi.mocked(invoke).mockResolvedValueOnce({ terminal: false, localeDefault: "fr" });

    const i18n = (await import("./i18n")).default;
    const { useUIStore } = await import("./store");
    const { getItem } = await import("./storage");
    await useUIStore.getState().loadHostFeatures();

    expect(i18n.language).toBe("en");
    expect(getItem("lang-host-default")).toBe("fr");
  });
});
