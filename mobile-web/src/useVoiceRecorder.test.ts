import { describe, expect, it, vi } from "vitest";

vi.mock("@capacitor/core", () => ({
  Capacitor: { isNativePlatform: () => false },
}));

const { formatDuration, pressIntent, TAP_MS, SPEAKING_WINDOW_MS } = await import(
  "./useVoiceRecorder"
);

describe("pressIntent", () => {
  it("single tap continues recording, finger can leave screen", () => {
    expect(pressIntent(TAP_MS - 1)).toBe("keep");
    expect(pressIntent(0)).toBe("keep");
  });

  it("press, speak, release to stop", () => {
    expect(pressIntent(TAP_MS + 1)).toBe("stop");
  });

  // Boundary leans to 'keep recording': while finger is on screen, user sees recording bar, can record longer
  // and has ✕ to exit; conversely, misclassifying as stop cuts a sentence in half with no escape.
  it("exactly at threshold counts as tap", () => {
    expect(pressIntent(TAP_MS)).toBe("keep");
  });
});

describe("formatDuration", () => {
  it("pad seconds to two digits", () => {
    expect(formatDuration(7)).toBe("0:07");
  });

  it("carry over to minutes", () => {
    expect(formatDuration(63)).toBe("1:03");
  });

  it("fractional seconds floor, no flickering display", () => {
    expect(formatDuration(7.9)).toBe("0:07");
  });

  it("negative numbers show no minus sign", () => {
    expect(formatDuration(-3)).toBe("0:00");
  });
});

describe("SPEAKING_WINDOW_MS", () => {
  // Waveform judges activeness by 'recent speech'. Window too short flickers between sentences (speech has
  // natural pauses), too long keeps moving after speaker stops.
  it("allow normal pause between sentences", () => {
    expect(SPEAKING_WINDOW_MS).toBeGreaterThanOrEqual(800);
    expect(SPEAKING_WINDOW_MS).toBeLessThanOrEqual(2000);
  });
});
