/** 浏览器后退栈。
 *
 *  移动端每一个全屏浮层（会话详情 / 知识库文档 / 仓库 / 用量 / 新会话 / 目录选择器）
 *  以及「当前不在主页 tab」这件事，都在这里登记一层。硬件返回键、手势返回、页面里的
 *  返回按钮三条路径因此共用同一个栈：返回按钮走 history.back()，回到 popstate 上来，
 *  和用户自己按返回没有区别。
 *
 *  记账模型：`applied` = 我们压进浏览器历史的条目数 = 哨兵(1) + 层数。
 *  push/drop 只改 `layers`，真正的 history 调用集中在 reconcile() 里，按
 *  desired(= layers.length + 1) 与 applied 的差值一次补齐。这样 React StrictMode 的
 *  mount → cleanup → mount 双跑会在同一个微任务里自相抵消（push 后立刻 drop 再 push，
 *  净变化为 0），不会残留半层历史让用户多按一次返回。
 *
 *  哨兵条目是栈底能拦住返回的前提：没有它，用户在主页按返回会直接卸载 document，
 *  popstate 根本不会派发到我们手里。 */

export interface HistoryLike {
  pushState(data: unknown, unused: string): void;
  go(delta: number): void;
}

/** 栈底（哨兵被消耗）时按返回的处理结果：
 *  - "hold" —— 拦下这次返回，重新压回哨兵（调用方负责给出「再按一次退出」之类的提示）。
 *  - "leave" —— 放行，真的离开页面。 */
export type RootBackResult = "hold" | "leave";

type Layer = { id: number; close: () => void };

export class NavStack {
  private layers: Layer[] = [];
  private nextId = 1;
  /** 我们压进历史的条目数，含哨兵。 */
  private applied = 0;
  /** 自己发起的 go(-n) 会回吐一次 popstate（跳 n 格是单次导航，只派发一个事件），
   *  这里记下要跳过的次数，免得把它当成用户按了返回而多关一层。 */
  private ignorePops = 0;
  private scheduled = false;
  private started = false;

  constructor(
    private history: HistoryLike,
    private onRootBack: () => RootBackResult,
    // 必须包一层：直接写 `= queueMicrotask` 会把它当裸函数存进实例字段，之后
    // this.schedule(...) 的 receiver 是 NavStack 实例，浏览器抛 "Illegal invocation"。
    private schedule: (fn: () => void) => void = (fn) => queueMicrotask(fn),
  ) {}

  /** 压入哨兵。必须在任何 push() 之前调用一次。 */
  start(): void {
    if (this.started) return;
    this.started = true;
    this.history.pushState({ fleet: 0 }, "");
    this.applied = 1;
  }

  /** 登记一层，返回用于注销的 id。`close` 在用户按返回弹掉这一层时被调用。 */
  push(close: () => void): number {
    const id = this.nextId++;
    this.layers.push({ id, close });
    this.reconcileSoon();
    return id;
  }

  /** 注销一层。两种来路：
   *  - UI 主动关闭（点返回按钮 / 点遮罩）：层还在 layers 里，reconcile 会 go(-1) 把
   *    历史条目一起收掉，保持历史深度与可见层数一致。
   *  - popstate 已经弹掉它、React 随后卸载组件：层已不在 layers 里，这里是 no-op。 */
  drop(id: number): void {
    const i = this.layers.findIndex((l) => l.id === id);
    if (i === -1) return;
    this.layers.splice(i, 1);
    this.reconcileSoon();
  }

  /** 挂到 window 的 popstate 上。 */
  handlePopState(): void {
    if (this.ignorePops > 0) {
      this.ignorePops--;
      return;
    }
    this.applied = Math.max(0, this.applied - 1);

    if (this.applied === 0) {
      // 哨兵被吃掉了 —— 用户在栈底按了返回。
      if (this.onRootBack() === "leave") {
        this.history.go(-1);
        return;
      }
      this.history.pushState({ fleet: 0 }, "");
      this.applied = 1;
      return;
    }

    // 弹掉栈顶：close() 会让 React 卸载该浮层，随之而来的 drop() 因为层已不在
    // layers 里而是 no-op，所以历史深度不会被重复回退。
    const top = this.layers.pop();
    top?.close();
  }

  /** 当前层数（不含哨兵）。 */
  get depth(): number {
    return this.layers.length;
  }

  private reconcileSoon(): void {
    if (this.scheduled) return;
    this.scheduled = true;
    this.schedule(() => {
      this.scheduled = false;
      this.reconcile();
    });
  }

  private reconcile(): void {
    if (!this.started) return;
    const desired = this.layers.length + 1; // +1 = 哨兵
    if (desired > this.applied) {
      for (let i = this.applied; i < desired; i++) this.history.pushState({ fleet: i }, "");
      this.applied = desired;
    } else if (desired < this.applied) {
      const delta = this.applied - desired;
      this.applied = desired;
      this.ignorePops++; // go(-delta) 只派发一次 popstate，无论 delta 多大
      this.history.go(-delta);
    }
  }
}
