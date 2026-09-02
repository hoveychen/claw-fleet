# mobile-web/scripts

## `fake-desktop.mjs` — 在本机端到端验移动端(尤其是多设备)

移动端很多行为只有**桌面端真的在 relay 那头**才会发生:决策卡合并、答复发给哪
一台、跨设备同 id 不串卡、下钻请求路由到哪一台、连接策略在后台的收敛。这些用
`?mock` 验不了(mock 没有 relay、没有第二台),而用真桌面端验又要两台开着 Fleet
的机器。

`fake-desktop.mjs` 就是那个缺口:它在 relay 上冒充一台桌面端 —— 自己派生
channel token、自己封解包、自己答请求 —— 所以**一台机器上可以同时起 N 台**。

只用 Node 内建能力(`WebSocket` 全局需 Node 22+、`node:crypto`),不引依赖。

### 三步跑起来

```bash
# 1. 本机 relay(仓库根目录)
RELAY_PORT=18099 RELAY_DATA_DIR=/tmp/relay-data cargo run -p fleet-relay

# 2. 两台「桌面端」，各一个 64 位十六进制 secret
node mobile-web/scripts/fake-desktop.mjs \
  --relay ws://127.0.0.1:18099 --secret $(printf 'a%.0s' {1..64}) --label mac   --cards 2
node mobile-web/scripts/fake-desktop.mjs \
  --relay ws://127.0.0.1:18099 --secret $(printf 'b%.0s' {1..64}) --label linux --cards 1

# 3. dev server
cd mobile-web && pnpm dev
```

然后在浏览器控制台把这两台写进设备簿(`devices.ts` 的存储格式),刷新页面:

```js
localStorage.setItem(
  "fleet-devices",
  JSON.stringify({
    devices: [
      { id: "d1", label: "公司 Mac",   secret: "a".repeat(64), relayBase: "http://127.0.0.1:18099", addedAt: 1 },
      { id: "d2", label: "家里 Linux", secret: "b".repeat(64), relayBase: "http://127.0.0.1:18099", addedAt: 2 },
    ],
    activeId: "d1",
  }),
);
```

### 参数

| 参数 | 默认 | 说明 |
|---|---|---|
| `--secret` | (必填) | 配对 secret。channelToken 与 encKey 都由它 HKDF 派生 |
| `--relay` | `ws://127.0.0.1:18099` | relay 的 WebSocket 基址 |
| `--label` | `fake` | 这台的名字:进 agent 指纹、workspace 名、会话标题 |
| `--cards` | `1` | 推几张决策卡。**几台实例的卡 id 故意相同**(g1、g2…) |
| `--sessions` | `3` | 推几条会话 |

### 几个踩过的坑

- **卡 id 刻意重复。**多台实例都从 `g1` 开始编号,因为「两台机器上同号的卡是
  两张不同的卡」是最容易写错、又最难靠单测覆盖的一条。答掉一台的 `g1` 之后,
  另一台的 `g1` 必须还在。
- **fixture 的形状必须对齐类型。**`GuardRequest` 少一个 `riskTags` 数组会在渲染
  时抛异常,而 React 会把**整棵树卸掉** —— 页面一片空白、控制台无错。会话少了
  `entrypoint` / `fleetSpawned` 则会被任务页整条筛掉(`types.ts::isFleetOwnedTask`)。
- **验可见性策略要用 CDP 打进页面主世界。** patchright 的 `evaluate` 跑在隔离
  世界,在那里改 `document.visibilityState` 页面自己看不见;而 headless Chromium
  的后台标签页也**不会**上报 hidden。正确姿势:
  `CDPSession.send("Runtime.evaluate", { expression: "Object.defineProperty(document,'visibilityState',{configurable:true,get:()=>'hidden'}); document.dispatchEvent(new Event('visibilitychange'))" })`。
- 协议的地面真相在三处:`fleet-relay/src/frames.rs`(信封)、
  `claw-fleet-core/src/relay_crypto.rs`(HKDF 参数)、`mobile-web/src/relay.ts`
  (业务帧)。改协议时它们与本脚本要一起改。
