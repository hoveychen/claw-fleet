// 语音输入的状态机。UI（VoiceButton）只管画，识别的生命周期都在这里。
//
// provider 由 currentVoiceProvider() 挑，本 hook 不认环境。三条实现在这一层之下
// 是等价的，所以 P4/P5 接进 Capacitor 和鸿蒙时这个文件不用动。

import { useCallback, useEffect, useRef, useState } from "react";
import { t } from "./i18n";
import type { VoiceErrorKind, VoiceSession } from "./voiceInput";
import { currentVoiceProvider } from "./voiceProviders";

export type VoiceState =
  /** 还在问 provider 能不能用。按钮此时不显示，避免闪一下又消失。 */
  | "probing"
  /** 这个环境没有可用的识别服务。按钮整个不出现。 */
  | "unsupported"
  | "idle"
  | "listening"
  | "error";

/** 出错时给用户看的话。没必要区分得太细 —— 用户能做的动作只有那么几种。 */
export function voiceErrorText(kind: VoiceErrorKind): string {
  switch (kind) {
    case "no-permission":
      return t("没有麦克风权限，请在系统设置里允许后重试");
    case "no-speech":
      return t("没听到声音");
    case "network":
      return t("语音识别服务连不上");
    case "unavailable":
      return t("这台设备没有可用的语音识别");
    case "aborted":
      return t("已取消");
  }
}

export interface UseVoiceInput {
  state: VoiceState;
  /** 说话过程中的实时回显，未定稿。 */
  partial: string;
  /** state 为 error 时的原因。 */
  error: VoiceErrorKind | null;
  start(): void;
  /** 停止收音，保留已识别的内容。 */
  stop(): void;
  /** 丢弃本次识别。 */
  cancel(): void;
}

/**
 * @param lang 识别语言，如 `zh-CN`。
 * @param onText 定稿的一段文字。一次识别可能回调多次（引擎自己断句）。
 */
export function useVoiceInput(lang: string, onText: (text: string) => void): UseVoiceInput {
  const [state, setState] = useState<VoiceState>("probing");
  const [partial, setPartial] = useState("");
  const [error, setError] = useState<VoiceErrorKind | null>(null);
  const sessionRef = useRef<VoiceSession | null>(null);

  // onText 每次渲染都是新函数（调用方基本都写内联箭头）。放进 ref，start 的
  // 回调就不必把它列进依赖，也不会因为父组件重渲染而作废一次进行中的识别。
  const onTextRef = useRef(onText);
  onTextRef.current = onText;

  // 开机探一次可用性。异步是因为原生侧要查权限和服务 —— 无 GMS 的国产安卓机上
  // Android 的 isRecognitionAvailable() 返回 false，只有问过才知道。
  useEffect(() => {
    let alive = true;
    const provider = currentVoiceProvider();
    if (!provider) {
      setState("unsupported");
      return;
    }
    void provider.isAvailable().then((ok) => {
      if (alive) setState(ok ? "idle" : "unsupported");
    });
    return () => {
      alive = false;
    };
  }, []);

  // 卸载时收掉进行中的识别，否则麦克风会一直开着。
  useEffect(() => {
    return () => {
      sessionRef.current?.cancel();
      sessionRef.current = null;
    };
  }, []);

  // 每次 start / stop / cancel 都换一代号。provider.start 是异步的，等它 resolve
  // 时用户可能已经取消了；比对代号就知道手上这次结果还算不算数。
  //
  // 不能改看 state：setState 是异步的，取消后紧接着 resolve 的那一帧里 state 仍是
  // "listening"，守卫会漏掉，麦克风就留在开着的状态。
  const genRef = useRef(0);

  const start = useCallback(() => {
    const provider = currentVoiceProvider();
    if (!provider || sessionRef.current) return;
    const gen = ++genRef.current;
    setError(null);
    setPartial("");
    setState("listening");
    void provider
      .start(lang, {
        // 三个回调都先验代号。web-speech provider 自己在 cancel 后就闭嘴了，
        // 但那是它的实现细节；这一层不该依赖每个 provider 都同样严谨 —— 漏一个
        // 就是「取消之后文字仍然往输入框里蹦」。
        onPartial: (text) => {
          if (gen !== genRef.current) return;
          setPartial(text);
        },
        onFinal: (text) => {
          if (gen !== genRef.current) return;
          setPartial("");
          onTextRef.current(text);
        },
        onError: (kind) => {
          if (gen !== genRef.current) return;
          sessionRef.current = null;
          setPartial("");
          // 用户自己取消的不是错误,静默回到待命。
          if (kind === "aborted") {
            setState("idle");
            return;
          }
          setError(kind);
          setState("error");
        },
      })
      .then((session) => {
        // 等它 resolve 的这段时间里用户已经取消了 —— 立刻把会话收掉,否则麦克风
        // 留在开着的状态,而 UI 早已回到待命,用户完全看不出来。
        if (gen !== genRef.current) {
          session.cancel();
          return;
        }
        sessionRef.current = session;
      })
      .catch(() => {
        if (gen !== genRef.current) return;
        sessionRef.current = null;
        setError("unavailable");
        setState("error");
      });
  }, [lang]);

  // 注意 stop **不**换代号:引擎在 stop 之后还要把最后一段定稿吐出来,换了代号
  // 那一段就被守卫挡掉了 —— 表现为「说完按停止,最后一句没进输入框」。
  // 只有 cancel(丢弃)和 start(新一轮作废旧的)才换。
  const stop = useCallback(() => {
    sessionRef.current?.stop();
    sessionRef.current = null;
    setPartial("");
    setState((s) => (s === "listening" ? "idle" : s));
  }, []);

  const cancel = useCallback(() => {
    genRef.current++;
    sessionRef.current?.cancel();
    sessionRef.current = null;
    setPartial("");
    setState((s) => (s === "listening" ? "idle" : s));
  }, []);

  return { state, partial, error, start, stop, cancel };
}
