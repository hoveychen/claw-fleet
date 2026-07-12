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
