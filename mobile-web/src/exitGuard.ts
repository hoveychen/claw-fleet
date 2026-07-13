import type { RootBackResult } from "./navStack";

/** 栈底返回的「再按一次退出」闸门。
 *
 *  为什么不只靠 beforeunload：iOS 独立 PWA（从主屏幕打开）下浏览器基本不弹那个原生
 *  对话框，而这正是 Fleet 移动端的主力形态——只挂 beforeunload 等于对最常用的场景没设防。
 *  所以真正拦住误触的是这里：第一次返回只弹提示并把哨兵压回去，2 秒内再按一次才放行。 */

export const EXIT_WINDOW_MS = 2_000;

export class ExitGuard {
  private armed = false;
  private timer: ReturnType<typeof setTimeout> | undefined;

  constructor(
    /** 驱动 toast 的显隐。 */
    private onArmedChange: (armed: boolean) => void,
    /** 放行前的收尾：摘掉 beforeunload——用户已经通过 toast 确认过一次意图了，
     *  再弹一个原生「离开此网站？」是第二次确认，纯属折磨。 */
    private onLeave: () => void,
    private windowMs: number = EXIT_WINDOW_MS,
  ) {}

  handleRootBack = (): RootBackResult => {
    if (this.armed) {
      this.disarm();
      this.onLeave();
      return "leave";
    }
    this.armed = true;
    this.onArmedChange(true);
    this.timer = setTimeout(() => this.disarm(), this.windowMs);
    return "hold";
  };

  private disarm(): void {
    if (this.timer !== undefined) clearTimeout(this.timer);
    this.timer = undefined;
    if (!this.armed) return;
    this.armed = false;
    this.onArmedChange(false);
  }
}

/** 刷新 / 关标签 / 地址栏跳走这些非返回路径的兜底确认（文案由浏览器决定，不可定制）。
 *  返回卸载函数。 */
export function installUnloadPrompt(): () => void {
  const onBeforeUnload = (e: BeforeUnloadEvent) => {
    e.preventDefault();
    e.returnValue = "";
  };
  window.addEventListener("beforeunload", onBeforeUnload);
  return () => window.removeEventListener("beforeunload", onBeforeUnload);
}
