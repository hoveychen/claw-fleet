import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@capacitor/core", () => ({
  Capacitor: { isNativePlatform: () => false },
}));

const { createTailGuard, TAIL_GRACE_MS } = await import("./voiceTail");

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("createTailGuard", () => {
  // 这就是「在决策卡『其他』里说完话、点停止，文字整段消失」的那条路径：
  // 引擎在 finish 之后没有再补一次定稿（鸿蒙的 end 事件先到就会拆掉回调 hook），
  // 于是最后那段只存在于实时回显里的话没有任何出口。
  it("停止后引擎没补定稿 → 到点把最后那段实时回显补交上去", () => {
    const commit = vi.fn();
    const g = createTailGuard(commit);
    g.partial("合一下 worktree");
    g.stop();
    expect(commit).not.toHaveBeenCalled(); // 先给引擎留出补定稿的时间
    vi.advanceTimersByTime(TAIL_GRACE_MS);
    expect(commit).toHaveBeenCalledExactlyOnceWith("合一下 worktree");
  });

  it("引擎补了定稿 → 不再补交，避免同一句话进两遍", () => {
    const commit = vi.fn();
    const g = createTailGuard(commit);
    g.partial("合一下");
    g.stop();
    g.final(); // 定稿走的是 useVoiceInput 自己那条路，这里只表示「它到了」
    vi.advanceTimersByTime(TAIL_GRACE_MS * 3);
    expect(commit).not.toHaveBeenCalled();
  });

  it("取消是丢弃 → 那段回显不该被补交", () => {
    const commit = vi.fn();
    const g = createTailGuard(commit);
    g.partial("不要这句");
    g.cancel();
    vi.advanceTimersByTime(TAIL_GRACE_MS * 3);
    expect(commit).not.toHaveBeenCalled();
  });

  it("一个字都没听到时停止 → 不补交空串", () => {
    const commit = vi.fn();
    const g = createTailGuard(commit);
    g.stop();
    vi.advanceTimersByTime(TAIL_GRACE_MS * 3);
    expect(commit).not.toHaveBeenCalled();
  });

  it("定稿到达后又来了新的一段回显，停止时补交的是新的那段", () => {
    const commit = vi.fn();
    const g = createTailGuard(commit);
    g.partial("第一句");
    g.final(); // 引擎自己断句，第一句已经进输入框了
    g.partial("第二句");
    g.stop();
    vi.advanceTimersByTime(TAIL_GRACE_MS);
    // 补交第一句就是把已经在输入框里的话再写一遍。
    expect(commit).toHaveBeenCalledExactlyOnceWith("第二句");
  });

  it("组件卸载后不再补交——输入框已经不在了", () => {
    const commit = vi.fn();
    const g = createTailGuard(commit);
    g.partial("说到一半页面被关了");
    g.stop();
    g.dispose();
    vi.advanceTimersByTime(TAIL_GRACE_MS * 3);
    expect(commit).not.toHaveBeenCalled();
  });

  // 停止之后引擎还在吐实时回显（web-speech 的 stop 是异步收尾）——补交的应该是
  // 用户在屏幕上最后看到的那一段，而不是按下停止那一瞬的旧值。
  it("停止后仍到达的回显会更新待补交的内容", () => {
    const commit = vi.fn();
    const g = createTailGuard(commit);
    g.partial("合一下 work");
    g.stop();
    g.partial("合一下 worktree");
    vi.advanceTimersByTime(TAIL_GRACE_MS);
    expect(commit).toHaveBeenCalledExactlyOnceWith("合一下 worktree");
  });
});

// 补交要 900ms 之后才落地。这段时间里如果输入框先把那段字抹掉、过一会儿再冒出来，
// 用户看到的仍然是「文字消失了」——只是短一点。所以守卫要告诉调用方「我还在等」，
// 界面据此把那段字留在屏幕上。
describe("createTailGuard 的等待态", () => {
  it("有待定稿时 stop 报「在等」，到点补交后报「等完了」", () => {
    const onSettle = vi.fn();
    const g = createTailGuard(vi.fn(), { onSettle });
    g.partial("最后一句");
    expect(g.stop()).toBe(true);
    expect(onSettle).not.toHaveBeenCalled();
    vi.advanceTimersByTime(TAIL_GRACE_MS);
    expect(onSettle).toHaveBeenCalledTimes(1);
  });

  it("一个字都没有时 stop 不进入等待态——界面不该多留一拍", () => {
    const onSettle = vi.fn();
    const g = createTailGuard(vi.fn(), { onSettle });
    expect(g.stop()).toBe(false);
  });

  it("定稿提前到达 → 立刻结束等待，不用干等到点", () => {
    const onSettle = vi.fn();
    const g = createTailGuard(vi.fn(), { onSettle });
    g.partial("最后一句");
    g.stop();
    g.final();
    expect(onSettle).toHaveBeenCalledTimes(1);
  });

  it("取消也结束等待", () => {
    const onSettle = vi.fn();
    const g = createTailGuard(vi.fn(), { onSettle });
    g.partial("最后一句");
    g.stop();
    g.cancel();
    expect(onSettle).toHaveBeenCalledTimes(1);
  });

  it("不在等待中时，定稿/取消不会白报一次「等完了」", () => {
    const onSettle = vi.fn();
    const g = createTailGuard(vi.fn(), { onSettle });
    g.partial("说着呢");
    g.final();
    g.cancel();
    expect(onSettle).not.toHaveBeenCalled();
  });
});
