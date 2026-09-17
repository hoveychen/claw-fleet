import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("@capacitor/core", () => ({
  Capacitor: { isNativePlatform: () => false },
}));

const { classifyWebSpeechError, webSpeechProvider } = await import("./voiceWebSpeech");

type Win = Record<string, unknown>;

/** A fake recognition engine that lets us manually feed events, replacing the
    window constructor. */
class FakeRecognition {
  static last: FakeRecognition | undefined;
  lang = "";
  continuous = false;
  interimResults = false;
  started = 0;
  stopped = 0;
  aborted = 0;
  onstart: (() => void) | null = null;
  onresult: ((e: unknown) => void) | null = null;
  onerror: ((e: { error?: string }) => void) | null = null;
  onend: (() => void) | null = null;

  constructor() {
    FakeRecognition.last = this;
  }
  start() {
    this.started++;
  }
  /** Engine truly opened the mic — spec's onstart callback. */
  begin() {
    this.onstart?.();
  }
  stop() {
    this.stopped++;
  }
  abort() {
    this.aborted++;
  }

  /** Feed a batch of results. `from` is this event's resultIndex. */
  emit(from: number, items: { text: string; final: boolean }[]): void {
    const results = items.map((it) => ({ isFinal: it.final, 0: { transcript: it.text } }));
    this.onresult?.({ resultIndex: from, results });
  }
}

function installEngine(): void {
  (window as unknown as Win)["SpeechRecognition"] = FakeRecognition;
}

/** Collect callback results for assertions. */
function collect() {
  const partial: string[] = [];
  const final: string[] = [];
  const errors: string[] = [];
  const ready: number[] = [];
  const ended: number[] = [];
  return {
    partial,
    final,
    errors,
    ready,
    ended,
    handlers: {
      onReady: () => ready.push(1),
      onPartial: (t: string) => partial.push(t),
      onFinal: (t: string) => final.push(t),
      onError: (k: string) => errors.push(k),
      onEnd: () => ended.push(1),
    },
  };
}

afterEach(() => {
  const w = window as unknown as Win;
  delete w["SpeechRecognition"];
  delete w["webkitSpeechRecognition"];
  FakeRecognition.last = undefined;
});

describe("classifyWebSpeechError", () => {
  it("both rejection types map to permission error", () => {
    expect(classifyWebSpeechError("not-allowed")).toBe("no-permission");
    expect(classifyWebSpeechError("service-not-allowed")).toBe("no-permission");
  });

  it("recognizes network, no-speech, and aborted", () => {
    expect(classifyWebSpeechError("network")).toBe("network");
    expect(classifyWebSpeechError("no-speech")).toBe("no-speech");
    expect(classifyWebSpeechError("aborted")).toBe("aborted");
  });

  it("unknown strings map to unavailable", () => {
    expect(classifyWebSpeechError("something-new")).toBe("unavailable");
    expect(classifyWebSpeechError(undefined)).toBe("unavailable");
  });
});

describe("webSpeechProvider", () => {
  it("no engine returns unavailable error, not exception", async () => {
    const c = collect();
    const s = await webSpeechProvider.start("zh-CN", c.handlers);
    expect(c.errors).toEqual(["unavailable"]);
    // Returned session must still be safe to call.
    expect(() => {
      s.stop();
      s.cancel();
    }).not.toThrow();
  });

  it("sets language and requests interim results on start", async () => {
    installEngine();
    const c = collect();
    await webSpeechProvider.start("zh-CN", c.handlers);
    const rec = FakeRecognition.last!;
    expect(rec.lang).toBe("zh-CN");
    expect(rec.continuous).toBe(true);
    expect(rec.interimResults).toBe(true);
    expect(rec.started).toBe(1);
  });

  it("separates interim and final results", async () => {
    installEngine();
    const c = collect();
    await webSpeechProvider.start("zh-CN", c.handlers);
    FakeRecognition.last!.emit(0, [{ text: "把 P3", final: false }]);
    FakeRecognition.last!.emit(0, [{ text: "把 P3 勾掉", final: true }]);
    expect(c.partial).toEqual(["把 P3"]);
    expect(c.final).toEqual(["把 P3 勾掉"]);
  });

  // results is cumulative. If we iterate from 0, the second event will re-report
  // the first segment's finalized content, appearing as duplicate text in the input.
  it("reports only new segments, no duplicates of finalized ones", async () => {
    installEngine();
    const c = collect();
    await webSpeechProvider.start("zh-CN", c.handlers);
    const rec = FakeRecognition.last!;
    rec.emit(0, [{ text: "第一段", final: true }]);
    // Second event: list now has two items, but resultIndex points to the second.
    rec.onresult?.({
      resultIndex: 1,
      results: [
        { isFinal: true, 0: { transcript: "第一段" } },
        { isFinal: true, 0: { transcript: "第二段" } },
      ],
    });
    expect(c.final).toEqual(["第一段", "第二段"]);
  });

  it("stop lets engine wrap up, finals still arrive", async () => {
    installEngine();
    const c = collect();
    const s = await webSpeechProvider.start("zh-CN", c.handlers);
    s.stop();
    expect(FakeRecognition.last!.stopped).toBe(1);
    // Engine sends its last segment only after stop — we must receive it.
    FakeRecognition.last!.emit(0, [{ text: "最后一句", final: true }]);
    expect(c.final).toEqual(["最后一句"]);
  });

  // cancel means "discard." After abort, the engine often sends one more aborted
  // onerror. If we report it, UI shows an error after user cancels, same mistake.
  it("cancel suppresses both engine errors and results afterward", async () => {
    installEngine();
    const c = collect();
    const s = await webSpeechProvider.start("zh-CN", c.handlers);
    s.cancel();
    expect(FakeRecognition.last!.aborted).toBe(1);
    FakeRecognition.last!.onerror?.({ error: "aborted" });
    FakeRecognition.last!.emit(0, [{ text: "不该出现", final: true }]);
    expect(c.errors).toEqual([]);
    expect(c.final).toEqual([]);
  });

  it("stops reporting results after error", async () => {
    installEngine();
    const c = collect();
    await webSpeechProvider.start("zh-CN", c.handlers);
    FakeRecognition.last!.onerror?.({ error: "network" });
    FakeRecognition.last!.emit(0, [{ text: "不该出现", final: true }]);
    expect(c.errors).toEqual(["network"]);
    expect(c.final).toEqual([]);
  });
});

// start() returning != mic opened. User speech during this window is lost, UI must
// say "preparing" not "listening", so onReady signal is a hard requirement.
describe("webSpeechProvider readiness signal", () => {
  it("not ready when start returns", async () => {
    installEngine();
    const c = collect();
    await webSpeechProvider.start("zh-CN", c.handlers);
    expect(c.ready).toEqual([]);
  });

  it("ready only after engine onstart", async () => {
    installEngine();
    const c = collect();
    await webSpeechProvider.start("zh-CN", c.handlers);
    FakeRecognition.last!.begin();
    expect(c.ready).toEqual([1]);
  });

  it("engine onstart after cancel doesn't report ready", async () => {
    installEngine();
    const c = collect();
    const s = await webSpeechProvider.start("zh-CN", c.handlers);
    s.cancel();
    FakeRecognition.last!.begin();
    expect(c.ready).toEqual([]);
  });
});

// Browser ends the session after long silence even with continuous=true (varies
// per implementation). Before, onend only set internal flag, page still thought
// it was recording — same symptom as harmony VAD wrapping up.
describe("engine self-shutdown", () => {
  it("onend must report to caller", async () => {
    installEngine();
    const c = collect();
    await webSpeechProvider.start("zh-CN", c.handlers);
    FakeRecognition.last!.begin();
    FakeRecognition.last!.onend?.();
    expect(c.ended).toHaveLength(1);
  });

  it("engine onend after cancel doesn't report", async () => {
    installEngine();
    const c = collect();
    const session = await webSpeechProvider.start("zh-CN", c.handlers);
    FakeRecognition.last!.begin();
    session.cancel();
    FakeRecognition.last!.onend?.();
    expect(c.ended).toHaveLength(0);
  });

  it("onend after error doesn't report", async () => {
    installEngine();
    const c = collect();
    await webSpeechProvider.start("zh-CN", c.handlers);
    FakeRecognition.last!.begin();
    FakeRecognition.last!.onerror?.({ error: "network" });
    FakeRecognition.last!.onend?.();
    expect(c.errors).toEqual(["network"]);
    expect(c.ended).toHaveLength(0);
  });
});
