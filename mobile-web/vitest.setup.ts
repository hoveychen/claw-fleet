// node 测试环境缺少一些浏览器全局，而部分模块在 import 期就会访问它们
// （i18n.ts 顶层读 localStorage + navigator.language）。node 自带一个实验性的
// localStorage 全局占了名字但未启用（需 --localstorage-file），所以这里用
// defineProperty 强制覆盖成可用的内存实现，让依赖它的单测能正常加载。
const store = new Map<string, string>();
Object.defineProperty(globalThis, "localStorage", {
  configurable: true,
  writable: true,
  value: {
    getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
    setItem: (k: string, v: string) => void store.set(k, v),
    removeItem: (k: string) => void store.delete(k),
    clear: () => store.clear(),
  },
});
if (!("navigator" in globalThis)) {
  Object.defineProperty(globalThis, "navigator", {
    configurable: true,
    writable: true,
    value: { language: "en" },
  });
}
// i18n.ts 顶层还读 window.location.hash（langFromHash),而 node 无 window。
// 提供最小 shim(含 location.hash + 定时器),让 import 期访问 window 的模块能加载。
// 各测试仍可在 beforeEach 用自己的 windowShim 覆盖它。
if (!("window" in globalThis)) {
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    writable: true,
    value: {
      location: { origin: "http://localhost", hash: "" },
      setTimeout: (fn: () => void, ms?: number) => setTimeout(fn, ms) as unknown as number,
      clearTimeout: (id: number) => clearTimeout(id),
      setInterval: (fn: () => void, ms?: number) => setInterval(fn, ms) as unknown as number,
      clearInterval: (id: number) => clearInterval(id),
    },
  });
}
