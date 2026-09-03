import { describe, expect, it } from "vitest";
import { applyCtrl, decodeOutput, encodeInput } from "./terminal";

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
