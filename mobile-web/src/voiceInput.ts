// Unified voice input interface and detection of which implementation fits this runtime.
//
// Three implementations, each standalone with distinct shape:
//   - Browser / PWA  → Web Speech API (`webkitSpeechRecognition`, uploads audio to vendor)
//   - Capacitor shell → @capgo/capacitor-speech-recognition (iOS SFSpeechRecognizer /
//                      Android SpeechRecognizer)
//   - HarmonyOS shell → `fleetNative` bridge → ArkTS Core Speech Kit (on-device, offline)
//
// **Detection order is the only thing truly critical here**, because Web Speech can't be
// feature-detected: Apple disabled it in iOS WKWebView but still hangs
// `webkitSpeechRecognition` on the window (WebKit #239816, unfixed for 3+ years).
// So `if (window.webkitSpeechRecognition)` is true in Capacitor's iOS shell, but
// `start()` never returns or errors — silent failure that looks like "nothing happened".
//
// Order must be **shell first, Web Speech last**: shell identity is certain (native bridge
// object exists or not, Capacitor runtime is or isn't there), while Web Speech is only
// trusted when "definitely not in any shell". The reverse would hit a dead-end in the iOS
// shell.
//
// Shell-side protocol reuses two existing channels, nothing new: web→shell is
// `window.fleetNative.x()` (nativeScan.ts), shell→web is named hooks + pending queue
// (nativePush.ts).

import { Capacitor } from "@capacitor/core";

/** 哪条实现在服务当前环境。 */
export type VoiceProviderId = "web-speech" | "capacitor" | "harmony";

/** 识别失败的原因。UI 只需要区分这几类来决定说什么。 */
export type VoiceErrorKind =
  /** 用户拒了麦克风 / 语音识别授权。可引导去设置里开。 */
  | "no-permission"
  /** 一直没听到人说话。正常情况，静默收场即可。 */
  | "no-speech"
  /** 识别服务连不上。Web Speech 在国内是常态（音频要发去厂商服务器）。 */
  | "network"
  /** 这台设备根本没有可用的识别服务，例如无 GMS 的国产安卓机。 */
  | "unavailable"
  /** 调用方自己取消的。 */
  | "aborted";

export interface VoiceHandlers {
  /**
   * The microphone actually started capturing audio.
   *
   * `start()` returning ≠ recording: all three implementations cross async first —
   * Web Speech waits for the browser to start recognition, Capacitor checks/requests
   * permissions, HarmonyOS needs `createEngine`. Speech during that gap is **lost**,
   * and the UI looks identical to "already recording", so the user only feels like
   * "the first half didn't register". With this signal, the UI can honestly say
   * "getting ready" before it's done.
   *
   * Every implementation must call it, and exactly once.
   */
  onReady(): void;
  /** Interim results while speaking, overwritten by later results. Used for live echo. */
  onPartial(text: string): void;
  /** Finalized text. One session may produce multiple segments (engine splits long audio). */
  onFinal(text: string): void;
  /** Error. After this, the session ends; no more callbacks. */
  onError(kind: VoiceErrorKind): void;
  /**
   * **The engine wrapped up on its own** — not because the caller asked.
   *
   * All three implementations have this moment, and it's not rare: HarmonyOS's VAD
   * decides it's done after 3s of silence (or 60s max recording), Web Speech closes
   * even with continuous=true after long silence, Capacitor resolves the `start()`
   * promise. Before, all three only marked the session dead internally — the page had
   * no idea, so the UI kept showing "listening" while the user spoke and got nothing,
   * only another tap on stop would get them out.
   *
   * Mutually exclusive with onError; one session has at most one ending. After the
   * caller cancels, this isn't reported.
   */
  onEnd(): void;
}

/** 一次进行中的识别。 */
export interface VoiceSession {
  /** 停止收音，把已经听到的定稿出来（还会再来一次 onFinal）。 */
  stop(): void;
  /** 丢弃本次结果，不再回调。 */
  cancel(): void;
}

export interface VoiceInputProvider {
  readonly id: VoiceProviderId;
  /**
   * 这台设备现在能不能真的识别。异步是因为原生侧要查权限和服务可用性 ——
   * 无 GMS 的国产安卓机上 Android 的 `isRecognitionAvailable()` 返回 false，
   * 只有问过才知道。
   */
  isAvailable(): Promise<boolean>;
  start(lang: string, handlers: VoiceHandlers): Promise<VoiceSession>;
  /**
   * 把用户送到能把麦克风权限打开的地方，并回答「现在授权了吗」。
   *
   * 可选,因为**只有原生壳做得到**:浏览器里没有任何 API 能打开站点权限设置,
   * 那条路只能给一句指引文案。所以这里没有「所有 provider 都实现一个空壳」的
   * 版本 —— 缺席就是缺席,UI 据此决定画一个真按钮还是一句话,不画按不动的按钮。
   *
   * 鸿蒙实现走 `requestPermissionOnSetting`:用户拒过一次之后
   * `requestPermissionsFromUser` 就再也不弹了,这个 API 是官方给的二次授权入口,
   * 而且它直接在应用内弹系统面板,比跳到设置里让用户自己找更短。
   */
  openPermissionSettings?(): Promise<boolean>;
}

/** 鸿蒙壳注入的原生桥对象名，与 nativeScan.ts 同一个。 */
const BRIDGE = "fleetNative";

/** 桥上语音相关的方法名。壳侧 methodList 要登记同名方法才算数。 */
const BRIDGE_START = "startVoice";

/**
 * 当前环境该用哪条实现，判不出来返回 null（例如桌面浏览器里没有 Web Speech 的
 * Firefox）。
 *
 * 注意这里判的都是**环境身份**，不是 Web API 的存在性：
 *   - 鸿蒙：桥上有没有登记 startVoice。壳没接语音时这一项为假，会继续往下走 ——
 *     所以老壳配新 web 不会选中一条不存在的实现。
 *   - Capacitor：Capacitor 运行时说自己在原生平台上。插件装没装另说
 *     （provider 自己的 isAvailable 会回答），但「在壳里」这件事是确定的。
 *   - 其余：浏览器 / PWA，此时 webkitSpeechRecognition 的存在性才可以采信。
 */
export function detectVoiceProvider(): VoiceProviderId | null {
  const w = window as unknown as Record<string, Record<string, unknown> | undefined>;
  if (typeof w[BRIDGE]?.[BRIDGE_START] === "function") return "harmony";
  if (Capacitor.isNativePlatform()) return "capacitor";
  if (hasWebSpeech()) return "web-speech";
  return null;
}

/**
 * 把识别出的一段文字并进输入框里已有的内容。
 *
 * 要不要补一个空格取决于接缝两侧是什么：中文之间加空格是错的，而两个英文词黏在
 * 一起同样是错的。Fleet 的语音内容天生中英混排（「把 P3 勾掉」「合一下 worktree」），
 * 所以两种情况在同一句话里都会出现 —— 只在接缝两侧都是 ASCII 字母/数字时才补。
 */
export function appendVoiceText(existing: string, addition: string): string {
  const add = addition.trim();
  if (!add) return existing;
  if (!existing) return add;
  const left = existing[existing.length - 1];
  // 已有内容自己就以空白收尾,再补一个就成了双空格。
  if (/\s/.test(left)) return existing + add;
  const wordish = /[A-Za-z0-9]/;
  return wordish.test(left) && wordish.test(add[0]) ? `${existing} ${add}` : existing + add;
}

/**
 * 浏览器有没有 Web Speech 的识别部分。
 *
 * **只有确认不在任何原生壳里之后才可以调这个** —— 见文件头，壳里它会说谎。
 */
export function hasWebSpeech(): boolean {
  const w = window as unknown as Record<string, unknown>;
  return (
    typeof w["SpeechRecognition"] === "function" ||
    typeof w["webkitSpeechRecognition"] === "function"
  );
}
