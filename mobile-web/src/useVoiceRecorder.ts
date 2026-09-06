// 录音条的状态机 —— VoiceButton / VoiceBar 只管画，这里管一次录音的全过程。
//
// 与底下那层 useVoiceInput 的分工：那层管 provider 的生命周期（开麦、拿结果、
// 报错），这层管**用户看到的那次录音**：文字往哪写、写了多少、录了多久、还能不能
// 反悔。区别体现在两件它必须记住的事上：
//
//   - `base`：这次录音**开始之前**输入框里的内容。有了它「重录」才可能——否则
//     用户想重说一遍，只能自己去输入框里手动删掉刚才识别错的那一段。
//   - `preview`：已定稿的文字 + 还在飘的那一段，合成一条给输入框直接显示。实时
//     转写就上在真正的输入框里（Gboard / ChatGPT 听写都是这样），而不是塞进一行
//     会被截断的小字。
//
// 语音的产物是文字，不是音频。所以这一层的每个决定都倒向「文字随时可改、可退」。

import { useCallback, useEffect, useRef, useState } from "react";
import { appendVoiceText, type VoiceErrorKind, type VoiceProviderId } from "./voiceInput";
import { useVoiceInput } from "./useVoiceInput";

/** 松手时长 ≤ 这个数算「点按」——录音继续，手指可以离开屏幕。 */
export const TAP_MS = 500;

/** 一次按压松手之后该怎么办。 */
export type PressIntent =
  /** 点按：进入（或留在）录音态，手指走开也继续录。 */
  | "keep"
  /** 长按：按住说话的用法，松手即收。 */
  | "stop";

/**
 * 松手时该干什么。只按时长分，没有别的维度。
 *
 * 老版本还有个「上滑取消」：盲手势、没有可视落点、用户无从知道滑多远算数
 * （连微信上滑都会浮出「取消 / 转文字」两个看得见的落点让你选）。取消现在是
 * 录音条上一个明确的 ✕。
 */
export function pressIntent(heldMs: number): PressIntent {
  return heldMs <= TAP_MS ? "keep" : "stop";
}

/** 录音时长的读法，`0:07` / `1:03`。 */
export function formatDuration(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

/** 距离最近一次听到内容多久还算「正在说」——波形靠它决定动不动。 */
export const SPEAKING_WINDOW_MS = 1200;

/**
 * 「停止并发送」按下之后，等最后一段定稿的时间。
 *
 * 引擎在 stop 之后**还会再吐一次定稿**（说完最后半句才收工）。立刻发出去的话，
 * 用户说的最后一句就丢了——而且丢得无声无息，人已经把手机放下了。所以这里停顿
 * 一下等它；期间每来一段定稿就重新等，直到安静下来才真的发。
 */
export const FINALIZE_MS = 1200;

export interface VoiceRecorderApi {
  /** 这台设备能不能语音输入。false 时调用方整个不该画语音入口。 */
  available: boolean;
  recording: boolean;
  /** 已经开始，但引擎还没说「麦克风真的开了」。这段空窗里说的话不会进任何
   *  结果，所以录音条不能装作已经在听（不走秒、不画波形）。 */
  preparing: boolean;
  /** 录音条该不该在屏幕上——收音结束后还有一小段等定稿的时间，那时条子不能
   *  先消失（否则用户按下发送后会看着一个什么都没发生的界面等一秒多）。 */
  active: boolean;
  /** 录音中输入框该显示的内容（已定稿 + 未定稿合成）。不录时等于原值。 */
  preview: string;
  /**
   * 输入框现在该显示 `preview` 而不是 `value`。
   *
   * **不等于 `recording`**：按下停止后还有一小段等最后一句定稿的时间，那段字仍在
   * `preview` 里而尚未进 `value`。这期间切回 `value`，用户看到的就是「文字消失了」
   * ——正是这一版要修的那个症状，只是短一点。
   */
  showingPreview: boolean;
  /** 未定稿的那一段，单独给出来是为了让调用方能把它画成灰的。 */
  partial: string;
  seconds: number;
  /** 最近 1.2 秒内听到了新内容。 */
  speaking: boolean;
  error: VoiceErrorKind | null;
  /** 这一轮录音已经往输入框里写进去过东西——决定要不要给「重录」。 */
  dirty: boolean;
  /** 调用方给了 onSend，录音条上才有「停止并发送」。 */
  canSend: boolean;
  /** 正在等最后一段定稿，随后就发出去。 */
  finalizing: boolean;
  start(): void;
  /** 收音结束，保留结果。 */
  stop(): void;
  /** 丢弃这次录音：连已经写进输入框的部分一起退回去。 */
  cancel(): void;
  /** 退回这次录音开始前的样子，然后重新开始录。 */
  retry(): void;
  /** 收工并把内容发出去。等最后一段定稿到齐才真的发。 */
  stopAndSend(): void;
  /** 关掉错误提示回到待命。 */
  dismissError(): void;
  /** 这个环境能不能把用户直接送去开麦克风权限（壳里能，浏览器不能）。 */
  canOpenSettings: boolean;
  /** 拉起授权入口；授权成功会自动接着开始录音。 */
  openSettings(): Promise<boolean>;
  /** 当前实现，错误提示靠它说清「权限在哪」。 */
  providerId: VoiceProviderId | null;
}

export function useVoiceRecorder({
  lang = "zh-CN",
  value,
  onChange,
  onSend,
}: {
  lang?: string;
  /** 输入框当前的内容。 */
  value: string;
  /** 语音要改写输入框时调这个。 */
  onChange: (next: string) => void;
  /** 给了才有「停止并发送」。省掉时录音条只有停止（决策卡里的输入框就是这样，
   *  那里的发送在卡片自己的按钮上）。 */
  onSend?: () => void;
}): VoiceRecorderApi {
  // 识别结果是异步到的，期间用户可能又敲了字；闭包里的 value 是那一刻的旧值，
  // 拿它拼接会把这几个字覆盖掉。两个都读 ref 里最近一次渲染的值。
  const valueRef = useRef(value);
  valueRef.current = value;
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;
  // 发送是在一个定时器里回调的，那时组件早已重渲染过好几轮；直接用捕获到的那个
  // 闭包会拿着旧的输入框内容去发 —— 正好丢掉刚等回来的最后一段定稿。
  const onSendRef = useRef(onSend);
  onSendRef.current = onSend;

  /** 这次录音开始前输入框里的内容。取消 / 重录都退回它。 */
  const baseRef = useRef<string | null>(null);
  const [dirty, setDirty] = useState(false);

  /** 「停止并发送」按下之后的等待。null = 没在等。 */
  const finalizeTimerRef = useRef<number | null>(null);
  const [finalizing, setFinalizing] = useState(false);

  const clearFinalize = useCallback(() => {
    if (finalizeTimerRef.current !== null) {
      window.clearTimeout(finalizeTimerRef.current);
      finalizeTimerRef.current = null;
    }
    setFinalizing(false);
  }, []);

  /** 重新开始数「安静了多久」。每来一段定稿就重来，直到真的没动静了才发。 */
  const armFinalize = useCallback(() => {
    if (finalizeTimerRef.current !== null) window.clearTimeout(finalizeTimerRef.current);
    setFinalizing(true);
    finalizeTimerRef.current = window.setTimeout(() => {
      finalizeTimerRef.current = null;
      setFinalizing(false);
      onSendRef.current?.();
    }, FINALIZE_MS);
  }, []);

  const voice = useVoiceInput(lang, (text) => {
    setDirty(true);
    onChangeRef.current(appendVoiceText(valueRef.current, text));
    // 还在等着发 —— 这段刚到的定稿说明引擎没说完，再等一轮。
    if (finalizeTimerRef.current !== null) armFinalize();
  });

  const recording = voice.state === "listening";
  // 收音停了、还在等最后一段定稿的那段时间。输入框在这期间要继续显示那段字
  // （见 voiceTail.ts），所以它和「正在录」一起决定输入框显示 preview 还是 value。
  const settling = voice.settling;
  const preparing = voice.state === "preparing";

  // 计时。只在录音时跑，停下就归零 —— 计时器是「这次录音」的属性。
  const [seconds, setSeconds] = useState(0);
  useEffect(() => {
    if (!recording) {
      setSeconds(0);
      return;
    }
    const started = Date.now();
    const id = window.setInterval(() => {
      setSeconds((Date.now() - started) / 1000);
    }, 250);
    return () => window.clearInterval(id);
  }, [recording]);

  // 「正在说」：靠识别结果的到达时间推断。真正的音量拿不到——Web Speech 不给，
  // 壳里的引擎自己占着麦克风，我们再开一路 getUserMedia 去测音量有抢麦的风险。
  // 所以波形是**活性指示**而不是音量表，而它挂靠的是一个真信号：引擎确实在出字。
  const [speaking, setSpeaking] = useState(false);
  const heardAtRef = useRef(0);
  useEffect(() => {
    if (voice.partial) heardAtRef.current = Date.now();
  }, [voice.partial]);
  useEffect(() => {
    if (!recording) {
      setSpeaking(false);
      return;
    }
    const id = window.setInterval(() => {
      setSpeaking(Date.now() - heardAtRef.current < SPEAKING_WINDOW_MS);
    }, 200);
    return () => window.clearInterval(id);
  }, [recording]);

  const start = useCallback(() => {
    clearFinalize();
    if (baseRef.current === null) {
      baseRef.current = valueRef.current;
      setDirty(false);
    }
    voice.start();
  }, [voice, clearFinalize]);

  const stop = useCallback(() => {
    baseRef.current = null;
    voice.stop();
  }, [voice]);

  const stopAndSend = useCallback(() => {
    baseRef.current = null;
    voice.stop();
    armFinalize();
  }, [voice, armFinalize]);

  // 组件被拆掉时别把一个待发送的定时器留在外面：它会对着已经卸载的输入框发送。
  useEffect(() => clearFinalize, [clearFinalize]);

  const cancel = useCallback(() => {
    clearFinalize();
    // 取消要连**已经写进输入框的定稿**一起退掉。引擎会把一段长话切成几段陆续
    // 定稿，只停收音的话，用户按下 ✕ 之前落下的那几句会留在输入框里 —— 那不是
    // 「取消」，那是「停止」。
    const base = baseRef.current;
    baseRef.current = null;
    voice.cancel();
    if (base !== null && base !== valueRef.current) onChangeRef.current(base);
    setDirty(false);
  }, [voice, clearFinalize]);

  const retry = useCallback(() => {
    clearFinalize();
    const base = baseRef.current ?? valueRef.current;
    voice.cancel();
    if (base !== valueRef.current) onChangeRef.current(base);
    setDirty(false);
    baseRef.current = base;
    voice.start();
  }, [voice, clearFinalize]);

  const dismissError = useCallback(() => {
    baseRef.current = null;
    voice.clearError();
  }, [voice]);

  // 授权成功就直接接着录。让用户授权完再自己回来点一次麦克风，是把一次操作
  // 拆成两次——而他刚刚做的那个动作，本意就是「我要说话」。
  const grantAndStart = useCallback(async (): Promise<boolean> => {
    const granted = await voice.openSettings();
    if (granted) start();
    return granted;
  }, [voice, start]);

  return {
    available: voice.state !== "probing" && voice.state !== "unsupported",
    recording,
    preparing,
    active: recording || preparing || finalizing || settling,
    showingPreview: recording || settling,
    preview: voice.partial ? appendVoiceText(value, voice.partial) : value,
    partial: voice.partial,
    seconds,
    speaking,
    error: voice.state === "error" ? voice.error : null,
    dirty,
    canSend: !!onSend,
    finalizing,
    start,
    stop,
    stopAndSend,
    cancel,
    retry,
    dismissError,
    canOpenSettings: voice.canOpenSettings,
    openSettings: grantAndStart,
    providerId: voice.providerId,
  };
}

/**
 * 录音时让输入框一直跟着最新的字走。
 *
 * 只读的 textarea 不会自己滚。说得长一点，用户就看着一个停在开头的框，以为没在
 * 识别 —— 而实时转写正是这次重做里「它听见了没有」的主要证据，看不见等于没有。
 */
export function useFollowTail<T extends HTMLElement>(
  active: boolean,
  content: unknown,
): React.RefObject<T | null> {
  const ref = useRef<T>(null);
  useEffect(() => {
    const el = ref.current;
    if (el && active) el.scrollTop = el.scrollHeight;
  }, [active, content]);
  return ref;
}
