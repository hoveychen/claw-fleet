import { describe, expect, it, vi } from "vitest";
import { NavStack, type HistoryLike, type RootBackResult } from "./navStack";

/** node 环境没有 window.history，用记账用的假实现注入。只记调用序列——
 *  NavStack 的正确性完全体现在「压了几条、退了几格」上。 */
function fakeHistory() {
  const calls: string[] = [];
  const history: HistoryLike = {
    pushState: () => void calls.push("push"),
    go: (d) => void calls.push(`go(${d})`),
  };
  return { history, calls };
}

/** 默认用同步 scheduler，断言不用等微任务；StrictMode 那条用例单独换成手动 flush。 */
function mk(onRootBack: () => RootBackResult = () => "hold") {
  const { history, calls } = fakeHistory();
  const stack = new NavStack(history, onRootBack, (fn) => fn());
  stack.start();
  calls.length = 0; // 丢掉哨兵那次 push，让后续断言只看层的动静
  return { stack, calls };
}

describe("NavStack", () => {
  it("start 压入哨兵，栈底的返回才有东西可吃", () => {
    const { history, calls } = fakeHistory();
    const stack = new NavStack(history, () => "hold", (fn) => fn());
    stack.start();
    expect(calls).toEqual(["push"]);
    // 重复 start 不再压第二条哨兵
    stack.start();
    expect(calls).toEqual(["push"]);
  });

  it("push 一层压一条历史；popstate 弹栈顶并调用它的 close", () => {
    const { stack, calls } = mk();
    const close = vi.fn();
    stack.push(close);
    expect(calls).toEqual(["push"]);

    stack.handlePopState();
    expect(close).toHaveBeenCalledTimes(1);
    expect(stack.depth).toBe(0);
    // 用户按的返回，浏览器已经退过了 —— 不该再自己 go()
    expect(calls).toEqual(["push"]);
  });

  it("popstate 弹掉一层后，React 卸载触发的 drop 不重复回退历史", () => {
    const { stack, calls } = mk();
    let id = 0;
    id = stack.push(() => stack.drop(id)); // 模拟 close → 组件卸载 → drop
    calls.length = 0;

    stack.handlePopState();
    expect(stack.depth).toBe(0);
    expect(calls).toEqual([]); // 关键：没有多余的 go(-1)
  });

  it("UI 主动关闭（drop）把对应的历史条目一起收掉，回吐的 popstate 不再关下一层", () => {
    const { stack, calls } = mk();
    const closeOuter = vi.fn();
    const closeInner = vi.fn();
    stack.push(closeOuter);
    const inner = stack.push(closeInner);
    calls.length = 0;

    stack.drop(inner); // 点了返回按钮
    expect(calls).toEqual(["go(-1)"]);

    // 浏览器随后回吐一个 popstate —— 必须被吸收，否则外层也会被误关
    stack.handlePopState();
    expect(closeOuter).not.toHaveBeenCalled();
    expect(stack.depth).toBe(1);

    // 之后用户真按返回，才轮到外层
    stack.handlePopState();
    expect(closeOuter).toHaveBeenCalledTimes(1);
  });

  it("多层时 popstate 只弹栈顶", () => {
    const { stack } = mk();
    const a = vi.fn();
    const b = vi.fn();
    stack.push(a);
    stack.push(b);

    stack.handlePopState();
    expect(b).toHaveBeenCalledTimes(1);
    expect(a).not.toHaveBeenCalled();
    expect(stack.depth).toBe(1);
  });

  it("StrictMode 的 push→drop→push 双跑在一个微任务里自相抵消，不残留半层历史", () => {
    const { history, calls } = fakeHistory();
    const pending: Array<() => void> = [];
    const stack = new NavStack(history, () => "hold", (fn) => void pending.push(fn));
    stack.start();
    calls.length = 0;

    // React 18/19 StrictMode：effect 跑 → cleanup → effect 再跑，全在提交阶段内
    const id1 = stack.push(vi.fn());
    stack.drop(id1);
    stack.push(vi.fn());

    pending.forEach((fn) => fn()); // 微任务落地
    expect(calls).toEqual(["push"]); // 只压一条，且没有 go(-1)
    expect(stack.depth).toBe(1);
  });

  it("栈底返回：hold 拦下并压回哨兵，leave 才真的离开", () => {
    const hold = mk(() => "hold");
    hold.stack.handlePopState(); // 吃掉哨兵
    expect(hold.calls).toEqual(["push"]); // 哨兵被压回来
    hold.calls.length = 0;
    hold.stack.handlePopState(); // 哨兵还在，可以再拦一次
    expect(hold.calls).toEqual(["push"]);

    const onRoot = vi.fn<() => RootBackResult>().mockReturnValue("leave");
    const leave = mk(onRoot);
    leave.stack.handlePopState();
    expect(onRoot).toHaveBeenCalledTimes(1);
    expect(leave.calls).toEqual(["go(-1)"]); // 放行：退出 document
  });

  it("有浮层时 popstate 不会打到栈底返回上", () => {
    const onRoot = vi.fn<() => RootBackResult>().mockReturnValue("hold");
    const { stack } = mk(onRoot);
    stack.push(vi.fn());

    stack.handlePopState(); // 关浮层
    expect(onRoot).not.toHaveBeenCalled();

    stack.handlePopState(); // 这次才是栈底
    expect(onRoot).toHaveBeenCalledTimes(1);
  });
});
