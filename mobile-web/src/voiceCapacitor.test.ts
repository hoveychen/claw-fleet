import { beforeEach, describe, expect, it, vi } from "vitest";

// Fake plugin: serves both error-code classification pure test cases (which never
// reach here) and lets "the end of one recognition session" be driven for real —
// the native plugin can't run under Node, but the contract between us (when listener
// and start's promise resolve) can be replayed under control.
const listeners: Record<string, (e: unknown) => void> = {};
let resolveStart: ((r: { matches?: string[] }) => void) | undefined;
let rejectStart: ((e: unknown) => void) | undefined;

vi.mock("@capgo/capacitor-speech-recognition", () => ({
  SpeechRecognition: {
    checkPermissions: async () => ({ speechRecognition: "granted" }),
    requestPermissions: async () => ({ speechRecognition: "granted" }),
    addListener: async (name: string, cb: (e: unknown) => void) => {
      listeners[name] = cb;
      return { remove: () => delete listeners[name] };
    },
    start: () =>
      new Promise<{ matches?: string[] }>((res, rej) => {
        resolveStart = res;
        rejectStart = rej;
      }),
    stop: async () => {},
  },
}));

const { classifyNativeError, capacitorVoiceProvider } = await import("./voiceCapacitor");

function collect() {
  const final: string[] = [];
  const errors: string[] = [];
  const ended: number[] = [];
  return {
    final,
    errors,
    ended,
    handlers: {
      onReady: () => {},
      onPartial: () => {},
      onFinal: (t: string) => final.push(t),
      onError: (k: string) => errors.push(k),
      onEnd: () => ended.push(1),
    },
  };
}

beforeEach(() => {
  for (const k of Object.keys(listeners)) delete listeners[k];
  resolveStart = undefined;
  rejectStart = undefined;
});

describe("classifyNativeError", () => {
  it("recognizes permission errors", () => {
    expect(classifyNativeError("ERROR_INSUFFICIENT_PERMISSIONS")).toBe("no-permission");
    expect(classifyNativeError("permission_denied")).toBe("no-permission");
    expect(classifyNativeError("not-allowed")).toBe("no-permission");
  });

  it("recognizes no-speech errors", () => {
    expect(classifyNativeError("ERROR_NO_MATCH")).toBe("no-speech");
    expect(classifyNativeError("ERROR_SPEECH_TIMEOUT")).toBe("no-speech");
  });

  it("recognizes network errors", () => {
    expect(classifyNativeError("ERROR_NETWORK")).toBe("network");
    expect(classifyNativeError("ERROR_NETWORK_TIMEOUT")).toBe("network");
    expect(classifyNativeError("ERROR_SERVER")).toBe("network");
  });

  it("recognizes unavailable service", () => {
    expect(classifyNativeError("ON_DEVICE_RECOGNITION_UNAVAILABLE")).toBe("unavailable");
  });

  // The two platforms use different error codes with no documented enum, so not
  // recognizing is normal. Better to say "speech recognition unavailable" than
  // misclassify a network problem as a permission error, sending user to settings.
  it("unknown codes map to unavailable, no guessing", () => {
    expect(classifyNativeError("ERROR_CLIENT")).toBe("unavailable");
    expect(classifyNativeError("")).toBe("unavailable");
    expect(classifyNativeError("某个没见过的码")).toBe("unavailable");
  });

  it("case insensitive", () => {
    expect(classifyNativeError("error_network")).toBe("network");
    expect(classifyNativeError("ERROR_NETWORK")).toBe("network");
  });
});

// Plugin start() resolves only **after entire recognition finishes** — that moment
// is "engine shut down," whether from user stop or native determining speech ended.
// Before, we only reported the final result, not that the session ended, so UI
// kept showing "listening."
describe("one recognition session ends", () => {
  it("native shutdown reports onEnd, final result still reports first", async () => {
    const c = collect();
    await capacitorVoiceProvider.start("zh-CN", c.handlers);
    resolveStart!({ matches: ["说完了"] });
    await Promise.resolve();
    await Promise.resolve();
    expect(c.final).toEqual(["说完了"]);
    expect(c.ended).toHaveLength(1);
  });

  it("native completion after user cancel doesn't report", async () => {
    const c = collect();
    const session = await capacitorVoiceProvider.start("zh-CN", c.handlers);
    session.cancel();
    resolveStart!({ matches: ["不要这句"] });
    await Promise.resolve();
    await Promise.resolve();
    expect(c.final).toEqual([]);
    expect(c.ended).toHaveLength(0);
  });

  it("error end reports error only, no extra onEnd", async () => {
    const c = collect();
    await capacitorVoiceProvider.start("zh-CN", c.handlers);
    rejectStart!(new Error("ERROR_NETWORK"));
    await Promise.resolve();
    await Promise.resolve();
    expect(c.errors).toEqual(["network"]);
    expect(c.ended).toHaveLength(0);
  });
});
