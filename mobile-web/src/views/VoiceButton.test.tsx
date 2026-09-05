import { describe, expect, it, vi } from "vitest";

vi.mock("@capacitor/core", () => ({
  Capacitor: { isNativePlatform: () => false },
}));

const { appendVoiceText } = await import("../voiceInput");
const { voiceHint } = await import("./VoiceButton");

// 交互从「按住说话 + 上滑取消 + 点按锁定」改成了点按开关，releaseIntent /
// TAP_MS / CANCEL_DY 连同它们的用例一起删了 —— 两个看不见的阈值没有了，
// 也就没有阈值判别可测。剩下真正会出错的规则是「哪个状态说哪句话」。

describe("voiceHint", () => {
  it("准备中要说出来，不能冒充正在听", () => {
    const preparing = voiceHint("preparing", "");
    expect(preparing).not.toBe("");
    expect(preparing).not.toBe(voiceHint("listening", ""));
  });

  // 麦克风还没开，这时哪怕引擎抢跑吐了字也不能显示成「已经在听你说」。
  it("准备中即使有残留 partial 也仍报准备中", () => {
    expect(voiceHint("preparing", "残留")).toBe(voiceHint("preparing", ""));
  });

  it("正在听且还没出字时给停止指引", () => {
    expect(voiceHint("listening", "")).not.toBe("");
  });

  it("出了字就显示识别内容", () => {
    expect(voiceHint("listening", "把 P3 勾掉")).toBe("把 P3 勾掉");
  });

  it("空闲 / 不可用 / 出错时不占这一行", () => {
    expect(voiceHint("idle", "")).toBe("");
    expect(voiceHint("unsupported", "")).toBe("");
    expect(voiceHint("probing", "")).toBe("");
    // 出错时这一行让位给错误文案，不能两句话挤在一起。
    expect(voiceHint("error", "说了一半")).toBe("");
  });
});

describe("appendVoiceText", () => {
  it("空输入框直接放进去", () => {
    expect(appendVoiceText("", "把 P3 勾掉")).toBe("把 P3 勾掉");
  });

  it("中文之间不补空格", () => {
    expect(appendVoiceText("先看一下", "这个问题")).toBe("先看一下这个问题");
  });

  // Fleet 的语音内容天生中英混排,两种接缝会出现在同一句话里。
  it("两个英文词之间补空格", () => {
    expect(appendVoiceText("merge the", "worktree")).toBe("merge the worktree");
  });

  it("中文接英文不补空格", () => {
    expect(appendVoiceText("合一下", "worktree")).toBe("合一下worktree");
  });

  it("已有内容以空白收尾时不再补,避免双空格", () => {
    expect(appendVoiceText("merge the ", "worktree")).toBe("merge the worktree");
  });

  it("识别结果首尾空白被去掉", () => {
    expect(appendVoiceText("", "  把 P3 勾掉  ")).toBe("把 P3 勾掉");
  });

  it("空的识别结果不改动输入框", () => {
    expect(appendVoiceText("已经打的字", "   ")).toBe("已经打的字");
  });
});
