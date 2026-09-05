// 鸿蒙壳的语音识别。ArkWeb 里 Web Speech API 不可用，能力只能由壳提供：
// web 调 `fleetNative.startVoice()`，结果经 `window.__fleetVoice` 推回来
// （壳侧是 mobile-harmony 的 WebShell.ets + common/SpeechAsr.ets）。
//
// 底下是 Core Speech Kit 的端侧模型：离线、免费、无配额，是三条 provider 里
// 唯一不需要联网的。代价是**只识别中文** —— 官方明确仅支持中文，所以
// 「合一下 worktree」这种中英混排里的英文标识符会是短板。这是这条路的既定
// 边界，不是可以在 web 侧修的东西。
//
// 与 __fleetShare / __fleetPushToken 那两个 hook 不同，这里不需要 pending 队列：
// 那两个是冷启动带进来的，必然早于 React effect；语音事件只在用户按下按钮之后
// 才产生，那时 hook 早已注册。

import type {
  VoiceErrorKind,
  VoiceHandlers,
  VoiceInputProvider,
  VoiceSession,
} from "./voiceInput";

/** 壳注入的桥对象名，与 nativeScan.ts 同一个。 */
const BRIDGE = "fleetNative";
/** 壳把识别事件推回页面的入口。 */
const HOOK = "__fleetVoice";

/** 壳侧 VoiceEvent 的形状（WebShell.ets）。 */
interface HarmonyVoiceEvent {
  kind: "ready" | "partial" | "final" | "error" | "end";
  text: string;
  code: string;
}

interface HarmonyBridge {
  startVoice?: (lang: string) => void;
  stopVoice?: () => void;
  cancelVoice?: () => void;
}

function bridge(): HarmonyBridge | undefined {
  return (window as unknown as Record<string, HarmonyBridge | undefined>)[BRIDGE];
}

/**
 * 壳侧错误码 → 我们的分类。
 *
 * `PERMISSION_DENIED` / `START_FAILED` 是 SpeechAsr.ets 自己定的两个串；其余是
 * Core Speech Kit 的数字错误码转成的字符串。数字码的含义没有公开枚举，所以一律
 * 归到 unavailable —— 与 Capacitor 那条同样的保守做法：宁可笼统，也不要把一个
 * 引擎内部错误说成没有权限、让用户白跑一趟设置。
 */
export function classifyHarmonyError(code: string): VoiceErrorKind {
  if (code === "PERMISSION_DENIED") return "no-permission";
  return "unavailable";
}

export const harmonyVoiceProvider: VoiceInputProvider = {
  id: "harmony",

  // 桥上有 startVoice 就算可用。壳侧的引擎是系统内置的端侧模型，没有「这台
  // 设备装没装识别服务」这一说，所以不必再问一次原生。
  async isAvailable(): Promise<boolean> {
    return typeof bridge()?.startVoice === "function";
  },

  async start(lang: string, handlers: VoiceHandlers): Promise<VoiceSession> {
    const b = bridge();
    if (typeof b?.startVoice !== "function") {
      handlers.onError("unavailable");
      return { stop: () => {}, cancel: () => {} };
    }

    const w = window as unknown as Record<string, unknown>;
    let dead = false;
    const teardown = () => {
      delete w[HOOK];
    };

    w[HOOK] = (ev: HarmonyVoiceEvent) => {
      if (dead) return;
      switch (ev.kind) {
        case "ready":
          handlers.onReady();
          break;
        case "partial":
          if (ev.text) handlers.onPartial(ev.text);
          break;
        case "final":
          if (ev.text) handlers.onFinal(ev.text);
          break;
        case "error":
          dead = true;
          teardown();
          handlers.onError(classifyHarmonyError(ev.code));
          break;
        case "end":
          // 引擎自己收工(VAD 判定说完了、或到了 maxAudioDuration)。定稿已经
          // 在此之前经 final 到过,这里只需拆掉 hook。
          dead = true;
          teardown();
          break;
      }
    };

    b.startVoice(lang);

    return {
      // stop 让引擎正常收尾,最后一段定稿仍会经 final 到达,所以这里不拆 hook ——
      // 拆了那一段就没人收,表现为「说完按停止,最后一句没进输入框」。
      stop: () => {
        if (dead) return;
        bridge()?.stopVoice?.();
      },
      cancel: () => {
        if (dead) return;
        dead = true;
        teardown();
        bridge()?.cancelVoice?.();
      },
    };
  },
};
