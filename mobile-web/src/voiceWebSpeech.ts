// 浏览器 / PWA 的语音识别，走 Web Speech API。
//
// 只服务「确定不在任何原生壳里」的环境 —— 判定归 voiceInput.ts::detectVoiceProvider，
// 这里不再自己认环境（壳里 webkitSpeechRecognition 会说谎，见那边的文件头）。
//
// 这条路的音频是**上传厂商服务器**识别的：Chrome 发去 Google，Safari 发去 Apple。
// 所以它在国内的安卓 Chrome 上大概率直接 network 错 —— 那不是 bug，是这条实现的
// 固有边界，UI 按 `network` 收场即可。真正的离线识别在另外两个 provider 里。

import type {
  VoiceErrorKind,
  VoiceHandlers,
  VoiceInputProvider,
  VoiceSession,
} from "./voiceInput";
import { hasWebSpeech } from "./voiceInput";

/** Web Speech 的构造函数，无前缀优先。 */
function ctor(): (new () => SpeechRecognitionLike) | undefined {
  const w = window as unknown as Record<string, unknown>;
  const c = w["SpeechRecognition"] ?? w["webkitSpeechRecognition"];
  return typeof c === "function" ? (c as new () => SpeechRecognitionLike) : undefined;
}

/** 我们用到的那部分 SpeechRecognition。lib.dom 里这套类型并非处处都有，自己写。 */
interface SpeechRecognitionLike {
  lang: string;
  continuous: boolean;
  interimResults: boolean;
  start(): void;
  stop(): void;
  abort(): void;
  onstart: (() => void) | null;
  onresult: ((e: SpeechResultEvent) => void) | null;
  onerror: ((e: { error?: string }) => void) | null;
  onend: (() => void) | null;
}

interface SpeechResultEvent {
  resultIndex: number;
  results: ArrayLike<{ isFinal: boolean; 0: { transcript: string } }>;
}

/**
 * 规范里的 error 串 → 我们的分类。
 *
 * `service-not-allowed` 和 `not-allowed` 都归到授权：前者是系统/浏览器策略拒绝
 * （iOS 上的 Chrome 就恒报这个），后者是用户拒了麦克风。对用户来说都是「去把
 * 权限打开」，分开说没有意义。
 */
export function classifyWebSpeechError(error: string | undefined): VoiceErrorKind {
  switch (error) {
    case "not-allowed":
    case "service-not-allowed":
      return "no-permission";
    case "no-speech":
      return "no-speech";
    case "network":
      return "network";
    case "aborted":
      return "aborted";
    case "audio-capture":
      return "unavailable";
    default:
      return "unavailable";
  }
}

export const webSpeechProvider: VoiceInputProvider = {
  id: "web-speech",

  // 构造函数在就算可用。这里问不出「厂商服务器连不连得上」——那要真开一次麦克风
  // 才知道，代价太大，留给 start() 的 network 错误去报。
  async isAvailable(): Promise<boolean> {
    return hasWebSpeech();
  },

  async start(lang: string, handlers: VoiceHandlers): Promise<VoiceSession> {
    const C = ctor();
    if (!C) {
      handlers.onError("unavailable");
      return { stop: () => {}, cancel: () => {} };
    }

    const rec = new C();
    rec.lang = lang;
    // continuous:说完一句不自动收工,由用户按停止。语音输入常常是一段话,
    // 引擎默认的「一句就结束」会把后半截吃掉。
    rec.continuous = true;
    rec.interimResults = true;

    // cancel 之后引擎仍会吐 onend(有的实现还会吐一次 aborted onerror)。调用方
    // 已经说了不要结果,所以这里之后的一切都咽掉,而不是把它当成一次错误报上去。
    let dead = false;

    // 规范里的 onstart 就是「音频采集已开始」，正是我们要的那一声。
    rec.onstart = () => {
      if (dead) return;
      handlers.onReady();
    };

    rec.onresult = (e) => {
      if (dead) return;
      // 只看本次事件新增的那几条:results 是累积的,从头遍历会把已经定稿过的
      // 段落重复报一遍。
      for (let i = e.resultIndex; i < e.results.length; i++) {
        const r = e.results[i];
        const text = r[0]?.transcript ?? "";
        if (!text) continue;
        if (r.isFinal) handlers.onFinal(text);
        else handlers.onPartial(text);
      }
    };

    rec.onerror = (e) => {
      if (dead) return;
      dead = true;
      handlers.onError(classifyWebSpeechError(e.error));
    };

    rec.onend = () => {
      dead = true;
    };

    rec.start();

    return {
      // stop:停止收音,但让引擎把最后一段定稿吐出来(还会再来一次 onresult)。
      stop: () => {
        if (dead) return;
        rec.stop();
      },
      cancel: () => {
        if (dead) return;
        dead = true;
        rec.abort();
      },
    };
  },
};
