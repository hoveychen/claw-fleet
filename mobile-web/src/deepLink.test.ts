import { beforeEach, describe, expect, it, vi } from "vitest";

// deepLink.ts only lives in native shells; both Capacitor modules are unavailable under node,
// so mock them entirely: `isNativePlatform` always true (otherwise onPairingLink short-circuits
// to no-op), and App gets its launch URL injected by each test case.
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

/** Run the cold start path once, get what the handler receives. */
async function deliver(url: string | undefined): Promise<unknown> {
  launchUrl.value = url;
  let received: unknown = "__never_called__";
  const unsubscribe = onPairingLink((paired) => {
    received = paired;
  });
  // getLaunchUrl is a promise, let its .then queue at the end of microtasks.
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

  // Core of this fix: the QR code host is determined by **desktop** region/custom config
  // (claw-fleet-core::relay_region + "Relay Address" editable in settings panel), so the
  // origin scanned is the relay this device should connect to. The shell previously only
  // took secret and discarded origin, so scanning a self-hosted relay's QR still connected
  // to the official relay baked in at build time—the symptom was just "always connect fails".
  // See the same pit's comment in Harmony shell's WebShell.ets.
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

  // Harmony shell explicitly writes `&relay=` (its page origin is fake domain fleet.local).
  // Android shell's App Link origin is already real, but explicit parameter describes
  // "this pairing session", more specific, so it takes priority — consistent with
  // relayBase.ts::resolveRelayBase priority.
  it("显式的 &relay= 胜过 URL 自身的 origin", async () => {
    const received = await deliver(
      `https://fleet.local/index.html#k=${SECRET}&relay=${encodeURIComponent("http://192.168.1.9:18080")}`,
    );
    expect(received).toEqual({ secret: SECRET, relayBase: "http://192.168.1.9:18080" });
  });

  // Links with no available origin (like custom scheme): must still pair, just not
  // naming a relay; in device registry becomes `relayBase: null` (relayBaseFor builds default).
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
