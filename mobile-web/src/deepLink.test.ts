import { beforeEach, describe, expect, it, vi } from "vitest";

// deepLink.ts 只在原生壳里活着，两个 Capacitor 模块在 node 下都不可用，所以
// 整个替掉：`isNativePlatform` 恒真（否则 onPairingLink 直接短路成 no-op），
// App 则由每个用例注入自己的启动 URL。
const launchUrl = { value: undefined as string | undefined };

vi.mock("@capacitor/core", () => ({
  Capacitor: { isNativePlatform: () => true },
}));
vi.mock("@capacitor/app", () => ({
  App: {
    getLaunchUrl: async () => (launchUrl.value ? { url: launchUrl.value } : null),
    addListener: async () => ({ remove: () => {} }),
  },
}));

const { onPairingLink } = await import("./deepLink");

/** 跑一次冷启动路径，拿到 handler 收到的东西。 */
async function deliver(url: string | undefined): Promise<unknown> {
  launchUrl.value = url;
  let received: unknown = "__never_called__";
  const unsubscribe = onPairingLink((paired) => {
    received = paired;
  });
  // getLaunchUrl 是 promise，让它的 .then 排到微任务队尾。
  await Promise.resolve();
  await Promise.resolve();
  unsubscribe();
  return received;
}

const SECRET = "b8c0de1f2a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5";

describe("onPairingLink", () => {
  beforeEach(() => {
    launchUrl.value = undefined;
  });

  // 这是本次修复的核心：二维码的 host 由**桌面端**所在地区/自建配置决定
  // （claw-fleet-core::relay_region + 设置面板里可改的「Relay 地址」），所以扫到
  // 的那个 origin 就是这台设备该连的 relay。壳此前只取 secret、把 origin 扔了，
  // 于是扫了自建 relay 的码照样去连打包时烧进去的官方 relay，现象只是「一直
  // 连不上」。参见鸿蒙壳 WebShell.ets 里同一个坑的注释。
  it("交出扫到的 relay origin，而不只是 secret", async () => {
    const received = await deliver(`https://relay.corp.example.com/#k=${SECRET}`);
    expect(received).toEqual({ secret: SECRET, relayBase: "https://relay.corp.example.com" });
  });

  it("官方 host 同样按扫到的 origin 走，而不是打包默认值", async () => {
    const received = await deliver(`https://fleet-relay.eternizedlab.com/#k=${SECRET}&lang=zh`);
    expect(received).toEqual({
      secret: SECRET,
      relayBase: "https://fleet-relay.eternizedlab.com",
    });
  });

  // 鸿蒙壳会显式写 `&relay=`（它的页面 origin 是假域名 fleet.local）。安卓壳走
  // App Link 时 origin 本身就是真的，但显式参数描述的是「这一次配对」，更具体，
  // 所以它优先——与 relayBase.ts::resolveRelayBase 的优先级保持一致。
  it("显式的 &relay= 胜过 URL 自身的 origin", async () => {
    const received = await deliver(
      `https://fleet.local/index.html#k=${SECRET}&relay=${encodeURIComponent("http://192.168.1.9:18080")}`,
    );
    expect(received).toEqual({ secret: SECRET, relayBase: "http://192.168.1.9:18080" });
  });

  // 自定义 scheme 之类没有可用 origin 的链接：仍然要能配对，只是没有指名
  // relay，落到设备簿里就是 `relayBase: null`（relayBaseFor 会给它构建默认值）。
  it("拿不到 http(s) origin 时 relayBase 为 null，但 secret 仍交出", async () => {
    const received = await deliver(`fleet://pair#k=${SECRET}`);
    expect(received).toEqual({ secret: SECRET, relayBase: null });
  });

  it("没有 #k= 的链接不触发配对", async () => {
    expect(await deliver("https://fleet-relay.muveeai.com/")).toBe("__never_called__");
  });

  it("没有启动 URL（普通点图标）不触发配对", async () => {
    expect(await deliver(undefined)).toBe("__never_called__");
  });
});
