// `?mock` 开关，单独一个零依赖模块。
//
// 它原本住在 mock/relay.ts 里，而那个文件 `extends RelayClient`。于是 App.tsx
// 为了读一个 query param，静态 import 了整棵 relay 依赖树 —— 同源构建里
// main.tsx 那对动态 import 消掉的东西，被这条链原样拖了回来（实测过：
// dist-webui 里能搜到 `fleet-relay/hkdf/v1` 和 `new WebSocket`）。
//
// 判断本身两行，跟 mock 数据毫无关系，所以它属于这里。

export function isMockMode(): boolean {
  return new URLSearchParams(window.location.search).has("mock");
}
