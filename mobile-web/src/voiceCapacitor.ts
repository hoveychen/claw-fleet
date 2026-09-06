// Capacitor 壳（iOS / Android）的语音识别。
//
// 底下是 iOS 的 SFSpeechRecognizer 和 Android 的 SpeechRecognizer，由
// @capgo/capacitor-speech-recognition 桥接。选这个包而不是自己写 Capacitor
// plugin：自己写要维护 Swift + Kotlin 两套原生代码，而它活跃维护、已合入标点
// 支持与分段会话，且同一家的 @capgo/capacitor-share-target 我们已经在用。
//
// **Android 上这条路依赖 GMS**：无 Google 服务的国产 ROM 上
// `SpeechRecognizer.isRecognitionAvailable()` 为 false，插件的 available() 会
// 如实返回 false，于是按钮整个不出现。这是既定的方案边界（不引云服务兜底），
// 不是 bug —— 见计划 voice-input-native 的取舍。

import { SpeechRecognition } from "@capgo/capacitor-speech-recognition";
import type {
  VoiceErrorKind,
  VoiceHandlers,
  VoiceInputProvider,
  VoiceSession,
} from "./voiceInput";

/**
 * 原生错误码 → 我们的分类。
 *
 * **插件没有文档化的错误码枚举**：`SpeechRecognitionErrorEvent.code` 是原生侧
 * 透传的字符串（Android 的 SpeechRecognizer 常量、iOS 的 NSError），两个平台
 * 各说各话。所以这里按关键词做尽力匹配，认不出的一律归到 unavailable ——
 * 宁可说得笼统，也不要把一个网络问题说成没有权限。
 *
 * 真机验收（P6）时要拿实际打出来的码值校准这张表。
 */
export function classifyNativeError(code: string): VoiceErrorKind {
  const c = code.toLowerCase();
  if (c.includes("permission") || c.includes("denied") || c.includes("not-allowed")) {
    return "no-permission";
  }
  if (c.includes("no_match") || c.includes("nomatch") || c.includes("speech_timeout")) {
    return "no-speech";
  }
  if (c.includes("network") || c.includes("server")) return "network";
  if (c.includes("unavailable") || c.includes("not_available")) return "unavailable";
  return "unavailable";
}

export const capacitorVoiceProvider: VoiceInputProvider = {
  id: "capacitor",

  async isAvailable(): Promise<boolean> {
    try {
      const { available } = await SpeechRecognition.available();
      return available;
    } catch {
      // 壳里插件没装好 / 原生侧抛了 —— 当作不可用，按钮不出现。
      return false;
    }
  },

  async start(lang: string, handlers: VoiceHandlers): Promise<VoiceSession> {
    // cancel 之后原生仍会把事件送完。调用方已经说了不要，这里之后一律咽掉。
    let dead = false;
    const listeners: { remove: () => void }[] = [];
    const teardown = () => {
      for (const l of listeners) l.remove();
      listeners.length = 0;
    };

    // 权限：先查再要。iOS 上这一项同时覆盖语音识别与麦克风两个授权。
    try {
      let status = await SpeechRecognition.checkPermissions();
      if (status.speechRecognition !== "granted") {
        status = await SpeechRecognition.requestPermissions();
      }
      if (status.speechRecognition !== "granted") {
        handlers.onError("no-permission");
        return { stop: () => {}, cancel: () => {} };
      }
    } catch {
      handlers.onError("no-permission");
      return { stop: () => {}, cancel: () => {} };
    }

    // 原生侧真的开始收音了才算就绪。'startingListening' 不算 —— 那一步还在
    // 起识别会话，此时说的话仍然会丢。
    listeners.push(
      await SpeechRecognition.addListener("listeningState", (e) => {
        if (dead) return;
        if (e.state === "started") handlers.onReady();
      }),
    );

    listeners.push(
      await SpeechRecognition.addListener("partialResults", (e) => {
        if (dead) return;
        // 连续 PTT 会话下 accumulatedText 是含本轮在内的全文，比 matches[0]
        // 完整；没有它再退回本轮的首条匹配。
        const text = e.accumulatedText ?? e.matches?.[0] ?? "";
        if (text) handlers.onPartial(text);
      }),
    );

    listeners.push(
      await SpeechRecognition.addListener("error", (e) => {
        if (dead) return;
        dead = true;
        teardown();
        handlers.onError(classifyNativeError(e.code ?? ""));
      }),
    );

    // **不要 await 这个 promise**：插件的 start() 要到整段识别结束才 resolve，
    // await 它就等于把 VoiceSession 扣到用户说完为止，UI 全程拿不到可以按停止
    // 的句柄。发起它、挂上收尾回调，立刻把 session 交出去。
    void SpeechRecognition.start({
      language: lang,
      partialResults: true,
      // iOS 16+ 的原生标点。语音输入的内容要直接进 prompt，没有标点很难读。
      addPunctuation: true,
    })
      .then(({ matches }) => {
        if (dead) return;
        dead = true;
        teardown();
        const text = matches?.[0];
        if (text) handlers.onFinal(text);
        // promise resolve 就是「这段识别结束了」,不管是用户按的停止还是原生
        // 自己判定说完了。上层据此收掉「正在听」。
        handlers.onEnd();
      })
      .catch((e: unknown) => {
        if (dead) return;
        dead = true;
        teardown();
        const code = e instanceof Error ? e.message : String(e);
        handlers.onError(classifyNativeError(code));
      });

    return {
      // stop 让原生正常收尾,上面的 .then 会拿到最终结果。
      stop: () => {
        if (dead) return;
        void SpeechRecognition.stop().catch(() => {});
      },
      cancel: () => {
        if (dead) return;
        dead = true;
        teardown();
        void SpeechRecognition.stop().catch(() => {});
      },
    };
  },
};
