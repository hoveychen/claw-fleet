import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("@capacitor/core", () => ({
  Capacitor: { isNativePlatform: () => false },
}));

const { classifyWebSpeechError, webSpeechProvider } = await import("./voiceWebSpeech");

type Win = Record<string, unknown>;

/** 一个可以手动喂事件的假引擎，替掉 window 上的构造函数。 */
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
  /** 引擎真的开麦了 —— 规范里的 onstart。 */
  begin() {
    this.onstart?.();
  }
  stop() {
    this.stopped++;
  }
  abort() {
    this.aborted++;
  }

  /** 喂一批结果。`from` 是本次事件的 resultIndex。 */
  emit(from: number, items: { text: string; final: boolean }[]): void {
    const results = items.map((it) => ({ isFinal: it.final, 0: { transcript: it.text } }));
    this.onresult?.({ resultIndex: from, results });
  }
}

function installEngine(): void {
  (window as unknown as Win)["SpeechRecognition"] = FakeRecognition;
}

/** 收集三种回调，供断言。 */
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
  it("两种拒绝都归到授权问题", () => {
    expect(classifyWebSpeechError("not-allowed")).toBe("no-permission");
    expect(classifyWebSpeechError("service-not-allowed")).toBe("no-permission");
  });

  it("认得网络、没听到、被取消", () => {
    expect(classifyWebSpeechError("network")).toBe("network");
    expect(classifyWebSpeechError("no-speech")).toBe("no-speech");
    expect(classifyWebSpeechError("aborted")).toBe("aborted");
  });

  it("没见过的串归到不可用", () => {
    expect(classifyWebSpeechError("something-new")).toBe("unavailable");
    expect(classifyWebSpeechError(undefined)).toBe("unavailable");
  });
});

describe("webSpeechProvider", () => {
  it("没有引擎时报 unavailable 而不是抛异常", async () => {
    const c = collect();
    const s = await webSpeechProvider.start("zh-CN", c.handlers);
    expect(c.errors).toEqual(["unavailable"]);
    // 返回的 session 仍要能安全调用。
    expect(() => {
      s.stop();
      s.cancel();
    }).not.toThrow();
  });

  it("开始识别时配好语言并要求中间结果", async () => {
    installEngine();
    const c = collect();
    await webSpeechProvider.start("zh-CN", c.handlers);
    const rec = FakeRecognition.last!;
    expect(rec.lang).toBe("zh-CN");
    expect(rec.continuous).toBe(true);
    expect(rec.interimResults).toBe(true);
    expect(rec.started).toBe(1);
  });

  it("中间结果与定稿分流", async () => {
    installEngine();
    const c = collect();
    await webSpeechProvider.start("zh-CN", c.handlers);
    FakeRecognition.last!.emit(0, [{ text: "把 P3", final: false }]);
    FakeRecognition.last!.emit(0, [{ text: "把 P3 勾掉", final: true }]);
    expect(c.partial).toEqual(["把 P3"]);
    expect(c.final).toEqual(["把 P3 勾掉"]);
  });

  // results 是累积列表。若从 0 遍历，第二段事件会把第一段已经定稿的内容再报一遍，
  // 表现为输入框里文字重复。
  it("只报本次新增的段落,不重复已定稿的", async () => {
    installEngine();
    const c = collect();
    await webSpeechProvider.start("zh-CN", c.handlers);
    const rec = FakeRecognition.last!;
    rec.emit(0, [{ text: "第一段", final: true }]);
    // 第二次事件:列表里累积了两条,但 resultIndex 指向第二条。
    rec.onresult?.({
      resultIndex: 1,
      results: [
        { isFinal: true, 0: { transcript: "第一段" } },
        { isFinal: true, 0: { transcript: "第二段" } },
      ],
    });
    expect(c.final).toEqual(["第一段", "第二段"]);
  });

  it("stop 让引擎收尾,定稿仍会到达", async () => {
    installEngine();
    const c = collect();
    const s = await webSpeechProvider.start("zh-CN", c.handlers);
    s.stop();
    expect(FakeRecognition.last!.stopped).toBe(1);
    // 引擎在 stop 之后才把最后一段吐出来 —— 这一段必须收下。
    FakeRecognition.last!.emit(0, [{ text: "最后一句", final: true }]);
    expect(c.final).toEqual(["最后一句"]);
  });

  // cancel 的语义是「丢弃」。引擎在 abort 之后往往还会补一记 aborted onerror，
  // 若照报上去，UI 会在用户主动取消后弹一条错误。
  it("cancel 之后引擎补的错误与结果都不再上报", async () => {
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

  it("出错后不再上报后续结果", async () => {
    installEngine();
    const c = collect();
    await webSpeechProvider.start("zh-CN", c.handlers);
    FakeRecognition.last!.onerror?.({ error: "network" });
    FakeRecognition.last!.emit(0, [{ text: "不该出现", final: true }]);
    expect(c.errors).toEqual(["network"]);
    expect(c.final).toEqual([]);
  });
});

// start() 返回 ≠ 麦克风已经开。这段空窗里用户说的话全丢，界面必须说「准备中」
// 而不是「正在听」，所以 onReady 是这条实现的硬要求。
describe("webSpeechProvider 的就绪信号", () => {
  it("start 返回时还没就绪", async () => {
    installEngine();
    const c = collect();
    await webSpeechProvider.start("zh-CN", c.handlers);
    expect(c.ready).toEqual([]);
  });

  it("引擎 onstart 之后才报就绪", async () => {
    installEngine();
    const c = collect();
    await webSpeechProvider.start("zh-CN", c.handlers);
    FakeRecognition.last!.begin();
    expect(c.ready).toEqual([1]);
  });

  it("cancel 之后引擎补的 onstart 不再上报", async () => {
    installEngine();
    const c = collect();
    const s = await webSpeechProvider.start("zh-CN", c.handlers);
    s.cancel();
    FakeRecognition.last!.begin();
    expect(c.ready).toEqual([]);
  });
});

// 浏览器即便 continuous=true 也会在长静默后自己结束会话（各家实现不一）。以前
// onend 只把内部的 dead 置上，页面还以为在录 —— 和鸿蒙 VAD 收工是同一个症状。
describe("引擎自己收工", () => {
  it("onend 要上报给调用方", async () => {
    installEngine();
    const c = collect();
    await webSpeechProvider.start("zh-CN", c.handlers);
    FakeRecognition.last!.begin();
    FakeRecognition.last!.onend?.();
    expect(c.ended).toHaveLength(1);
  });

  it("取消之后引擎补的 onend 不上报", async () => {
    installEngine();
    const c = collect();
    const session = await webSpeechProvider.start("zh-CN", c.handlers);
    FakeRecognition.last!.begin();
    session.cancel();
    FakeRecognition.last!.onend?.();
    expect(c.ended).toHaveLength(0);
  });

  it("报错收场之后紧跟的 onend 不再上报", async () => {
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
