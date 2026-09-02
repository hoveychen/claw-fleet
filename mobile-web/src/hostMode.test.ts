import { describe, expect, it } from "vitest";
import mainSrc from "./main.tsx?raw";
import pushSrc from "./push.ts?raw";
import appSrc from "./App.tsx?raw";

// 「webui 产物里不含 relay」这条约束有个很难看见的失效方式：写法上明明是动态
// import、逻辑上明明不可达，Rollup 却照样落出一个 relay-*.js chunk。实测过两
// 次：第一次是 main.tsx 用 hostMode 的 IS_WEBUI 做条件，第二次是 push.ts 用
// SUPPORTS_PUSH 守卫那条 `import("./relay")` —— 两次产物里都能搜到
// `resolveRelayBase` 和 `fleet-relay/hkdf/v1`。
//
// 原因是 vite 的 define 只替换**字面出现**的 `import.meta.env.VITE_FLEET_HOST`。
// 经过一层 const 中转后 Rollup 就不折叠了，而不折叠就不消 chunk。
//
// 所以这几处对折叠敏感的条件必须写成 define 的原表达式。这条测试守的就是这个
// ——它查源码文本，因为没有任何行为测试会因为「产物里多了一个 chunk」变红。
// 真正的产物验证在 P3 的构建检查里（grep dist-webui）；这条是它的早期哨兵。
describe("同源构建不得含 relay —— 对常量折叠敏感的几处写法", () => {
  it("main.tsx 选传输层时直接用 define 的表达式，不经 hostMode 中转", () => {
    expect(mainSrc).toContain('import.meta.env.VITE_FLEET_HOST === "webui"');
    // 反向也钉住：用 IS_WEBUI 做这个判断会让 relay chunk 复活。
    expect(mainSrc).not.toMatch(/IS_WEBUI\s*[?&|]/);
  });

  // push.ts 从前为了取 VAPID 公钥而动态 import 整个 relay 客户端，并用 define
  // 的原表达式守着它好让 Rollup 消掉。多设备之后 VAPID 是**每个 relay 各自**
  // 一把，地址由调用方给，于是那条 import 整个不存在了 —— 比守着它更彻底。
  // 剩下要钉的就是它别再长回来：`./relayBase` 是允许的（纯叶子模块，无模块加载
  // 期副作用、不含 WebSocket 与加密），`./relay` 及 `./relayCrypto` 不允许。
  it("push.ts 不得 import relay 客户端（只允许 relayBase 这个纯叶子）", () => {
    const imports = [...pushSrc.matchAll(/from\s+"([^"]+)"/g)].map((m) => m[1]);
    const dynamic = [...pushSrc.matchAll(/import\(\s*"([^"]+)"/g)].map((m) => m[1]);
    const relayish = [...imports, ...dynamic].filter((spec) => /(^|\/)relay/i.test(spec));
    expect(relayish).toEqual(["./relayBase"]);
  });

  // App.tsx 曾经为了读一个 query param 而 import mock/relay.ts，而那个文件
  // extends RelayClient —— 一条静态链就把整棵 relay 依赖树拖进了同源产物。
  it("App.tsx 不得静态 import 任何 relay 侧模块", () => {
    const imports = [...appSrc.matchAll(/from\s+"([^"]+)"/g)].map((m) => m[1]);
    expect(imports.filter((s) => /relay/i.test(s))).toEqual([]);
  });
});
