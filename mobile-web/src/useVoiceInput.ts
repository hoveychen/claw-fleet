// 语音输入的状态机。UI（VoiceButton）只管画，识别的生命周期都在这里。
//
// provider 由 currentVoiceProvider() 挑，本 hook 不认环境。三条实现在这一层之下
// 是等价的，所以 P4/P5 接进 Capacitor 和鸿蒙时这个文件不用动。

import { useCallback, useEffect, useRef, useState } from "react";
import { t } from "./i18n";
import type { VoiceErrorKind, VoiceProviderId, VoiceSession } from "./voiceInput";
import { currentVoiceProvider } from "./voiceProviders";
import { holdWakeLock } from "./wakeLock";

export type VoiceState =
  /** 还在问 provider 能不能用。按钮此时不显示，避免闪一下又消失。 */
  | "probing"
  /** 这个环境没有可用的识别服务。按钮整个不出现。 */
  | "unsupported"
  | "idle"
  /**
   * 已经发起，但麦克风还没真的开始收音。
   *
   * 三条实现都要先跨一段异步（起识别会话 / 查权限 / createEngine），这段空窗里
   * 用户说的话全丢。以前这一段被画成「正在听」，于是用户只会觉得「前半句怎么
   * 没识别出来」。分出这一态，界面才能老实说「准备中」。
   */
  | "preparing"
  | "listening"
  | "error";

/** 出错时给用户看的话。没必要区分得太细 —— 用户能做的动作只有那么几种。 */
export function voiceErrorText(kind: VoiceErrorKind): string {
  switch (kind) {
    case "no-permission":
      // 「请在系统设置里允许」这半句挪到 voiceErrorHint 去了:能把用户送过去的
      // 环境根本不需要这句话(按钮就是那个动作),送不过去的环境则需要说清楚是
      // *哪个*设置 —— 浏览器和壳里根本不是同一个地方。
      return t("没有麦克风权限");
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

/**
 * 错误提示的第二行：**这一刻用户能做什么**。
 *
 * 为什么要分环境写：「请在系统设置里允许麦克风」在浏览器里是错的（那是站点权限，
 * 不在系统设置里），在能直接拉起授权面板的壳里则是多余的（旁边就有按钮）。一条
 * 指向错地方的指引比不给指引更糟 —— 用户会真的翻一遍设置，然后回来发现没用。
 *
 * @param canOpenSettings 这个环境能不能直接把用户送去授权（壳里能，浏览器不能）。
 * @param providerId 当前实现，决定「权限在哪」这句话怎么说。
 */
export function voiceErrorHint(
  kind: VoiceErrorKind,
  canOpenSettings: boolean,
  providerId: VoiceProviderId | null,
): string | null {
  if (kind === "no-permission") {
    // 有按钮就别再写字了,按钮本身就是那句指引。
    if (canOpenSettings) return null;
    return providerId === "web-speech"
      ? t("在浏览器地址栏左侧的站点设置里，把麦克风改成「允许」")
      : t("到系统设置 → 应用 → Fleet → 权限里允许麦克风");
  }
  if (kind === "network") return t("语音识别要把声音发去厂商服务器，检查下网络");
  if (kind === "unavailable") return t("换成打字，或在能用的设备上说");
  return null;
}

/**
 * 这台设备能不能语音输入。
 *
 * 单独抽出来是因为**有两处必须用同一个判断**：语音按钮（不可用就整个不出现）
 * 和输入框的 placeholder（「发消息或按住说话」）。两边各写一套的话，无 GMS 的
 * 国产安卓机上就会出现「提示说能按住说话，按钮却不在」——一句指向不存在的
 * 控件的说明，比不提更糟。
 *
 * 异步是因为原生侧要查权限和服务：Android 的 isRecognitionAvailable() 在无
 * Google 服务的 ROM 上返回 false，只有问过才知道。
 */
export function useVoiceAvailable(): "probing" | "ready" | "unsupported" {
  const [status, setStatus] = useState<"probing" | "ready" | "unsupported">("probing");
  useEffect(() => {
    let alive = true;
    const provider = currentVoiceProvider();
    if (!provider) {
      setStatus("unsupported");
      return;
    }
    void provider.isAvailable().then((ok) => {
      if (alive) setStatus(ok ? "ready" : "unsupported");
    });
    return () => {
      alive = false;
    };
  }, []);
  return status;
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
  /** 收掉错误提示回到待命。cancel 不做这件事：它只管收麦克风，报出去的错该
   *  留在屏幕上，直到用户自己看过。 */
  clearError(): void;
  /** 这个环境能不能把用户送去开麦克风权限。false 时 UI 只能给一句指引文案，
   *  不该画一个按下去什么都不会发生的按钮。 */
  canOpenSettings: boolean;
  /** 拉起授权入口，返回用户是否授权了。canOpenSettings 为 false 时恒 false。 */
  openSettings(): Promise<boolean>;
  /** 当前在用哪条实现。错误提示要靠它说清「权限在哪」。 */
  providerId: VoiceProviderId | null;
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

  // 开机探一次可用性。
  const probe = useVoiceAvailable();
  useEffect(() => {
    if (probe === "probing") return;
    setState((s) => (s === "probing" ? (probe === "ready" ? "idle" : "unsupported") : s));
  }, [probe]);

  // 录音全程强制常亮，不管用户的常亮开关开没开。
  //
  // 手机默认几十秒无触摸就自动息屏，而说话时手根本不碰屏幕——说一段长一点的话
  // 必然撞上。屏一灭，WebView 被系统挂起，识别会话当场断掉：用户抬头一看，应用
  // 像是被杀掉了，刚才说的全没了，而且没有任何提示。所以这不是「顺手加个体验」，
  // 是这条路径能不能用完的问题。
  //
  // preparing 也算在内：那一段正是用户已经开口、引擎还没开麦的空窗，息屏同样致命。
  // 松开时机跟着 state 走，出错 / 停止 / 卸载都会走到这个 effect 的清理。
  useEffect(() => {
    if (state !== "listening" && state !== "preparing") return;
    return holdWakeLock();
  }, [state]);

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
    setState("preparing");
    void provider
      .start(lang, {
        onReady: () => {
          if (gen !== genRef.current) return;
          setState((s) => (s === "preparing" ? "listening" : s));
        },
        // 三个回调都先验代号。web-speech provider 自己在 cancel 后就闭嘴了，
        // 但那是它的实现细节；这一层不该依赖每个 provider 都同样严谨 —— 漏一个
        // 就是「取消之后文字仍然往输入框里蹦」。
        // 出字了就一定已经在收音 —— 万一某条实现漏了 onReady，这里兜住，
        // 不至于把界面永远钉在「准备中」。
        onPartial: (text) => {
          if (gen !== genRef.current) return;
          setState((s) => (s === "preparing" ? "listening" : s));
          setPartial(text);
        },
        onFinal: (text) => {
          if (gen !== genRef.current) return;
          setState((s) => (s === "preparing" ? "listening" : s));
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
    setState((s) => (s === "listening" || s === "preparing" ? "idle" : s));
  }, []);

  const cancel = useCallback(() => {
    genRef.current++;
    sessionRef.current?.cancel();
    sessionRef.current = null;
    setPartial("");
    setState((s) => (s === "listening" || s === "preparing" ? "idle" : s));
  }, []);

  const clearError = useCallback(() => {
    setError(null);
    setState((s) => (s === "error" ? "idle" : s));
  }, []);

  // 只有原生壳实现得了(浏览器没有打开站点权限设置的 API)。探测放在渲染期而不是
  // state 里:provider 是同步取的,而这个值只影响错误块画按钮还是画一句话。
  const provider = currentVoiceProvider();
  const canOpenSettings = typeof provider?.openPermissionSettings === "function";
  const providerId = provider?.id ?? null;

  const openSettings = useCallback(async (): Promise<boolean> => {
    const provider = currentVoiceProvider();
    if (typeof provider?.openPermissionSettings !== "function") return false;
    const granted = await provider.openPermissionSettings();
    // 授权成功了就把错误撤掉 —— 那条提示已经不成立了,留着只会让用户以为没生效。
    if (granted) {
      setError(null);
      setState((s) => (s === "error" ? "idle" : s));
    }
    return granted;
  }, []);

  return {
    state,
    partial,
    error,
    start,
    stop,
    cancel,
    clearError,
    canOpenSettings,
    openSettings,
    providerId,
  };
}
