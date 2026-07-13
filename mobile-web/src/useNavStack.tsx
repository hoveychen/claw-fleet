import { useEffect, useRef } from "react";
import { NavStack, type RootBackResult } from "./navStack";

/** NavStack 的 React 薄壳。
 *
 *  栈必须是模块级懒单例，不能挂在 App 的 effect 里：React 的 effect 是子先于父跑的，
 *  浮层（子）注册自己那一层时，App（父）的 effect 还没执行。 */

let stack: NavStack | undefined;
/** 栈底返回的处置权在 App（它才知道当前 tab 和「再按一次退出」的状态）。 */
let rootBackHandler: () => RootBackResult = () => "leave";

function getStack(): NavStack | undefined {
  if (typeof window === "undefined") return undefined; // 单测里 import 到也不炸
  if (!stack) {
    stack = new NavStack(window.history, () => rootBackHandler());
    stack.start();
    // 整个 app 生命周期都要听，不解绑。
    window.addEventListener("popstate", () => stack?.handlePopState());
  }
  return stack;
}

export function setRootBackHandler(fn: () => RootBackResult): void {
  rootBackHandler = fn;
  getStack(); // 顺手确保哨兵已压入
}

/** 组件挂载 = 打开一层，卸载 = 关掉一层。用户按返回时 `onBack` 被调用，由它去改
 *  React 状态把浮层关掉；反过来点页面里的返回按钮直接改状态即可，卸载时这里会把
 *  对应的历史条目一并收掉——两个方向都收敛到同一套记账。 */
export function useHistoryLayer(onBack: () => void): void {
  const ref = useRef(onBack);
  ref.current = onBack;
  useEffect(() => {
    const s = getStack();
    if (!s) return;
    const id = s.push(() => ref.current());
    return () => s.drop(id);
  }, []);
}

/** 给没有独立组件的「层」用（比如「当前不在主页 tab」）：条件渲染它就等于登记一层。 */
export function HistoryLayer({ onBack }: { onBack: () => void }): null {
  useHistoryLayer(onBack);
  return null;
}
