import { describe, expect, it, vi } from "vitest";

vi.mock("@capacitor/core", () => ({
  Capacitor: { isNativePlatform: () => false },
}));

const { releaseIntent, CANCEL_DY, TAP_MS } = await import("./VoiceButton");
const { appendVoiceText } = await import("../voiceInput");

describe("releaseIntent", () => {
  it("快速点一下是锁定继续录,不是结束", () => {
    expect(releaseIntent(0, TAP_MS - 1)).toBe("lock");
  });

  it("按住说完松手是结束", () => {
    expect(releaseIntent(0, TAP_MS + 1)).toBe("stop");
  });

  it("上滑够远就是取消", () => {
    expect(releaseIntent(-CANCEL_DY, 1000)).toBe("cancel");
  });

  // 上滑取消要压过时长判断:手指已经移开按钮了,按了多久都不该留下结果。
  // 若先判时长,一次「快速上滑」会被当成点按而锁定录音 —— 用户以为取消了,
  // 麦克风却还开着。
  it("上滑取消压过点按", () => {
    expect(releaseIntent(-CANCEL_DY - 10, 10)).toBe("cancel");
  });

  it("轻微上滑不算取消", () => {
    expect(releaseIntent(-(CANCEL_DY - 1), 1000)).toBe("stop");
  });

  it("下滑不取消", () => {
    expect(releaseIntent(CANCEL_DY * 2, 1000)).toBe("stop");
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
