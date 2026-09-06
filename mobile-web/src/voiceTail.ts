// 停止收音时，那段**还没定稿的实时回显**该怎么办。
//
// 背景：识别引擎把一次说话切成若干段，每段先以 partial 实时回显、随后以 final
// 定稿。只有 final 会被写进输入框，partial 只是屏幕上飘着的字。用户按下停止时，
// 最后那一段往往还停在 partial 上 —— 它能不能活下来，全看引擎在收尾之后**还给不
// 给**一次 final。
//
// 而这件事并不可靠：鸿蒙壳里 `end`（引擎收工）事件一到就会拆掉回调 hook，只要它
// 排在最后那次 final 前面，那段字就没有任何出口。在决策卡的「其他」里说一句短话，
// 整句常常从头到尾只是一段 partial —— 于是点下停止，文字整段消失，像没输入过。
// 「有一定概率」正是因为它取决于原生那两个回调谁先到。
//
// 所以这里不再赌引擎会补定稿：停止后留一个宽限窗口，定稿到了就当它接管（用它的，
// 不重复写），到点还没来就把用户最后看到的那段字自己补交上去。**宁可多补一段可能
// 略糙的文字，也不能让用户说的话凭空消失** —— 前者用户看得见、可以改，后者他连
// 发生了什么都不知道。

/**
 * 停止之后给引擎补定稿留多久。
 *
 * 比 useVoiceRecorder 的 FINALIZE_MS（1200ms，「停止并发送」等定稿的时间）**短**：
 * 这样补交的文字还赶得上那次发送，不会出现「刚发出去的消息缺最后一句」。
 */
export const TAIL_GRACE_MS = 900;

export interface TailOptions {
  /** 覆盖宽限时长（测试用）。 */
  graceMs?: number;
  /**
   * 等待结束（补交了 / 定稿来了 / 被取消）。
   *
   * 界面靠它把「还没定稿的那段字」从屏幕上收走。补交要等将近一秒，这段时间里
   * 如果先把字抹掉、过一会儿再冒出来，用户看到的仍然是「文字消失了」，只是短一点。
   */
  onSettle?: () => void;
}

export interface TailGuard {
  /** 收到一段实时回显。 */
  partial(text: string): void;
  /** 收到一段定稿 —— 它接管了当前这段，待补交的作废。 */
  final(): void;
  /**
   * 用户按了停止：开始等定稿，到点没等到就补交。
   *
   * @returns 是不是真的进入了等待（有东西要等才算）。界面据此决定要不要多留一拍。
   */
  stop(): boolean;
  /** 用户按了取消 / 丢弃这次识别：什么都不补。 */
  cancel(): void;
  /** 组件卸载：停掉计时器，别对着已经不在的输入框写字。 */
  dispose(): void;
}

/**
 * @param commit 补交一段文字，语义与 onFinal 的定稿完全一样（调用方分不出来，
 *               也不该分得出来）。
 */
export function createTailGuard(
  commit: (text: string) => void,
  opts: TailOptions = {},
): TailGuard {
  const graceMs = opts.graceMs ?? TAIL_GRACE_MS;
  /** 当前这一段的实时回显；定稿一到就清空（那一段已经有出口了）。 */
  let pending = "";
  let timer: ReturnType<typeof setTimeout> | null = null;
  let dead = false;

  /** stop 之后、还没有结果的那段时间。 */
  let awaiting = false;

  const disarm = () => {
    if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
  };

  /** 结束等待并知会界面。不在等待中时是空操作 —— 别白报一次。 */
  const settle = () => {
    disarm();
    if (!awaiting) return;
    awaiting = false;
    opts.onSettle?.();
  };

  return {
    partial(text) {
      if (dead) return;
      pending = text;
      // 停止之后仍可能飘来更长的回显（web-speech 的 stop 是异步收尾）。计时器
      // 不重置：补交的时机跟着「按下停止」走，内容跟着「用户最后看到的」走。
    },
    final() {
      if (dead) return;
      pending = "";
      settle();
    },
    stop() {
      if (dead) return false;
      disarm();
      if (!pending) return false;
      awaiting = true;
      timer = setTimeout(() => {
        timer = null;
        const text = pending;
        pending = "";
        awaiting = false;
        if (text) commit(text);
        opts.onSettle?.();
      }, graceMs);
      return true;
    },
    cancel() {
      if (dead) return;
      pending = "";
      settle();
    },
    dispose() {
      dead = true;
      pending = "";
      awaiting = false;
      disarm();
    },
  };
}
