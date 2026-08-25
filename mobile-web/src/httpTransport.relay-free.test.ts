import { describe, expect, it } from "vitest";
// `?raw` 而不是 node:fs —— 这个 tsconfig 只带 vite/client 的类型,没有 node 的。
import src from "./httpTransport.ts?raw";

// 「webui 不依赖 relay」这条约束靠人看代码守不住:它坏掉的方式是某天有人在
// httpTransport.ts 里顺手 `import { 某个常量 } from "./relay"`,类型检查通过、
// 单测通过、UI 也照常跑 —— 唯一的后果是浏览器构建里多了一个 relay 客户端,
// 它在模块加载时就会执行 resolveRelayBase() 去解析一个自己永远不会用的地址。
// 没有任何一条行为测试会因此变红。
//
// 所以这条守卫查的是源码文本而不是行为。它刻意查得笨:相邻的
// `./relayCrypto`、`./relayHttpBase` 之类同样是 relay 侧的东西,一并拦住。
describe("httpTransport 与 relay 的隔离", () => {
  it("httpTransport.ts 不得 import 任何 relay 侧模块", () => {
    const imports = [...src.matchAll(/from\s+"([^"]+)"/g)].map((m) => m[1]);

    const relayish = imports.filter((spec) => /(^|\/)relay/i.test(spec));

    expect(relayish).toEqual([]);
    // 顺带钉住它确实在用共享层 —— 一个什么都不 import 的文件也能让上面那条
    // 通过,而那说明它把错误分层自己重造了一遍。
    expect(imports).toContain("./transport");
  });
});
