import { describe, expect, it } from "vitest";
import {
  applyCtrl,
  createOutputPump,
  decodeOutput,
  encodeInput,
  type ProcOutputChunk,
  type ProcRecord,
} from "./terminal";

/** 一个假的 pty 日志：按 offset 切增量，响应可以拖慢。 */
function fakeHost(log: string) {
  const record: ProcRecord = {
    id: "p1",
    workspacePath: "/w",
    command: "",
    status: "running",
    startedMs: 0,
    cols: 80,
    rows: 24,
  };
  let pending: Array<() => void> = [];
  return {
    read(offset: number | null): Promise<ProcOutputChunk> {
      const from = offset ?? 0;
      const slice = log.slice(from);
      return new Promise((resolve) => {
        // 响应挂在这里，由测试决定什么时候回 —— 模拟慢链路。
        pending.push(() =>
          resolve({ dataB64: btoa(slice), nextOffset: from + slice.length, record }),
        );
      });
    },
    /** 让所有在途请求一起回，然后把微任务跑干净。 */
    async flush() {
      const fire = pending;
      pending = [];
      for (const f of fire) f();
      await Promise.resolve();
      await Promise.resolve();
    },
    get inFlight() {
      return pending.length;
    },
  };
}

describe("encodeInput", () => {
  it("survives non-ASCII input", () => {
    // 直接 btoa("中") 会抛 InvalidCharacterError（btoa 只吃 latin1）。IME 上屏的
    // 整段中文、emoji 都走这条路，编错了就是静默丢键。
    expect(() => encodeInput("中文")).not.toThrow();
    expect(decodeOutput(encodeInput("中文 🚀"))).toEqual(
      new TextEncoder().encode("中文 🚀"),
    );
  });

  it("keeps control bytes byte-exact", () => {
    // Ctrl-C 必须原样是 0x03；被 UTF-8 或转义动过手脚就打断不了命令。
    expect(decodeOutput(encodeInput("\x03"))).toEqual(new Uint8Array([3]));
  });
});

describe("createOutputPump", () => {
  it("不因为响应慢于轮询间隔就把同一段输出写两遍", async () => {
    // 手机经 relay 打回桌面端，一次 proc_output 往返常常超过 300ms 的轮询间隔。
    // offset 只在 await 回来之后才推进，所以第二个 tick 会带着同一个 offset 出去，
    // 同一段 pty 回显被写两遍 —— 屏幕上 `ls` 显示成 `llss`。
    const host = fakeHost("ls");
    const written: string[] = [];
    const pump = createOutputPump({
      read: host.read,
      write: (bytes) => written.push(new TextDecoder().decode(bytes)),
    });

    void pump.poll(); // tick 1：出去了，没回来
    void pump.poll(); // tick 2：300ms 后又一个 tick
    await host.flush();

    expect(written.join("")).toBe("ls");
  });

  it("上一轮回来之后照常继续推进 offset", async () => {
    const host = fakeHost("abc");
    const written: string[] = [];
    const pump = createOutputPump({
      read: host.read,
      write: (bytes) => written.push(new TextDecoder().decode(bytes)),
    });

    void pump.poll();
    await host.flush();
    void pump.poll(); // 已经读到尾了，这轮应该是空的
    await host.flush();

    expect(written.join("")).toBe("abc");
  });

  it("stop 之后在途响应不再上屏", async () => {
    const host = fakeHost("x");
    const written: string[] = [];
    const pump = createOutputPump({
      read: host.read,
      write: (bytes) => written.push(new TextDecoder().decode(bytes)),
    });

    void pump.poll();
    pump.stop();
    await host.flush();

    expect(written).toEqual([]);
  });
});

describe("applyCtrl", () => {
  it("folds letters into control codes regardless of case", () => {
    expect(applyCtrl("c")).toBe("\x03");
    expect(applyCtrl("C")).toBe("\x03");
    expect(applyCtrl("a")).toBe("\x01");
    expect(applyCtrl("z")).toBe("\x1a");
  });

  it("covers the punctuation controls a soft keyboard can reach", () => {
    expect(applyCtrl("[")).toBe("\x1b");
    expect(applyCtrl("@")).toBe("\x00");
  });

  it("passes through what Ctrl has no meaning for", () => {
    // 吞掉会让人以为按键丢了 —— 原样发过去，最坏也只是一个普通字符。
    expect(applyCtrl("1")).toBe("1");
    expect(applyCtrl("中")).toBe("中");
    // 键位条自己发的完整序列不是单字符，绝不能被折。
    expect(applyCtrl("\x1b[A")).toBe("\x1b[A");
  });
});
