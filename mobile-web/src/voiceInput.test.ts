import { afterEach, describe, expect, it, vi } from "vitest";

// Capacitor runtime is unavailable under node, so we replace it entirely.
// Each test case controls `isNativePlatform` itself (module-level mock can only be set once,
// so we expose it via a mutable object).
const native = { value: false };
vi.mock("@capacitor/core", () => ({
  Capacitor: { isNativePlatform: () => native.value },
}));

const { appendVoiceText, detectVoiceProvider, hasWebSpeech } = await import("./voiceInput");

type Win = Record<string, unknown>;

/** Make window look like a browser that provides Web Speech. */
function giveWebSpeech(): void {
  (window as unknown as Win)["webkitSpeechRecognition"] = function () {};
}

/** Attach the native bridge for Harmony shell. `voice` determines whether this shell has voice support. */
function giveHarmonyBridge(opts: { voice: boolean }): void {
  const bridge: Win = { scanPairing: () => {} };
  if (opts.voice) bridge["startVoice"] = () => {};
  (window as unknown as Win)["fleetNative"] = bridge;
}

afterEach(() => {
  const w = window as unknown as Win;
  delete w["webkitSpeechRecognition"];
  delete w["SpeechRecognition"];
  delete w["fleetNative"];
  native.value = false;
});

describe("detectVoiceProvider", () => {
  it("uses Web Speech if browser provides it", () => {
    giveWebSpeech();
    expect(detectVoiceProvider()).toBe("web-speech");
  });

  it("returns null if browser has no Web Speech", () => {
    expect(detectVoiceProvider()).toBeNull();
  });

  // This is why this module exists. On iOS WKWebView, Apple disabled recognition but
  // still exposes `webkitSpeechRecognition` (WebKit #239816), so that object in the shell
  // is a trap: selecting it gives a start() that never returns or errors. Detection must
  // check the shell first.
  it("does not choose web-speech in Capacitor shell even if webkitSpeechRecognition exists", () => {
    native.value = true;
    giveWebSpeech();
    expect(detectVoiceProvider()).toBe("capacitor");
  });

  it("chooses Harmony bridge if it has voice support, ignoring Capacitor and Web Speech", () => {
    giveHarmonyBridge({ voice: true });
    native.value = true;
    giveWebSpeech();
    expect(detectVoiceProvider()).toBe("harmony");
  });

  // Old shell with new web: bridge exists but startVoice isn't registered. Can't choose harmony —
  // the implementation doesn't exist in the shell, calls would silently fail.
  it("does not choose Harmony when bridge has no voice support, falls through to next option", () => {
    giveHarmonyBridge({ voice: false });
    giveWebSpeech();
    expect(detectVoiceProvider()).toBe("web-speech");
  });

  it("returns null when Harmony bridge has no voice and Web Speech is unavailable", () => {
    giveHarmonyBridge({ voice: false });
    expect(detectVoiceProvider()).toBeNull();
  });
});

describe("hasWebSpeech", () => {
  it("recognizes unprefixed SpeechRecognition", () => {
    (window as unknown as Win)["SpeechRecognition"] = function () {};
    expect(hasWebSpeech()).toBe(true);
  });

  it("returns false when neither variant exists", () => {
    expect(hasWebSpeech()).toBe(false);
  });
});

// Originally lived in VoiceButton.test.tsx; when that component was replaced by the
// recording bar, these test cases moved here with the function they test.
describe("appendVoiceText", () => {
  it("puts text directly into empty input", () => {
    expect(appendVoiceText("", "把 P3 勾掉")).toBe("把 P3 勾掉");
  });

  it("does not add space between Chinese characters", () => {
    expect(appendVoiceText("先看一下", "这个问题")).toBe("先看一下这个问题");
  });

  // Fleet voice content naturally mixes Chinese and English, with both types of boundaries in the same sentence.
  it("adds space between English words", () => {
    expect(appendVoiceText("merge the", "worktree")).toBe("merge the worktree");
  });

  it("does not add space between Chinese and English", () => {
    expect(appendVoiceText("合一下", "worktree")).toBe("合一下worktree");
  });

  it("does not add space when existing content already ends with whitespace", () => {
    expect(appendVoiceText("merge the ", "worktree")).toBe("merge the worktree");
  });

  it("trims whitespace from recognition result", () => {
    expect(appendVoiceText("", "  把 P3 勾掉  ")).toBe("把 P3 勾掉");
  });

  it("does not modify input when recognition result is empty", () => {
    expect(appendVoiceText("已经打的字", "   ")).toBe("已经打的字");
  });
});
