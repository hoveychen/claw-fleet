import { describe, expect, it, vi } from "vitest";

vi.mock("@capacitor/core", () => ({
  Capacitor: { isNativePlatform: () => false },
}));

const { formatDuration, pressIntent, TAP_MS, SPEAKING_WINDOW_MS } = await import(
  "./useVoiceRecorder"
);

describe("pressIntent", () => {
  it("点一下是继续录，手指可以离开屏幕", () => {
    expect(pressIntent(TAP_MS - 1)).toBe("keep");
    expect(pressIntent(0)).toBe("keep");
  });

  it("按住说完松手是收工", () => {
    expect(pressIntent(TAP_MS + 1)).toBe("stop");
  });

  // 边界归到「继续录」这边：手指还压在屏幕上时用户看得见录音条，多录一会儿
  // 有 ✕ 可退；反过来误判成 stop 会把一句话截半，且没有退路。
  it("刚好卡在阈值上算点按", () => {
    expect(pressIntent(TAP_MS)).toBe("keep");
  });
});

describe("formatDuration", () => {
  it("秒补两位", () => {
    expect(formatDuration(7)).toBe("0:07");
  });

  it("过分钟进位", () => {
    expect(formatDuration(63)).toBe("1:03");
  });

  it("小数向下取整，不会跳着显示", () => {
    expect(formatDuration(7.9)).toBe("0:07");
  });

  it("负数不出现减号", () => {
    expect(formatDuration(-3)).toBe("0:00");
  });
});

describe("SPEAKING_WINDOW_MS", () => {
  // 波形靠「最近有没有出字」判活性。窗口太短会在两句之间闪断（说话本来就有
  // 停顿），太长则人已经不说了它还在动。
  it("留出一句话之间的正常停顿", () => {
    expect(SPEAKING_WINDOW_MS).toBeGreaterThanOrEqual(800);
    expect(SPEAKING_WINDOW_MS).toBeLessThanOrEqual(2000);
  });
});
