// 语音输入的统一接口，以及「这个运行环境该用哪条实现」的判定。
//
// 三条实现各自独立，形态完全不同：
//   - 浏览器 / PWA  → Web Speech API（`webkitSpeechRecognition`，音频上传厂商服务器）
//   - Capacitor 壳  → @capgo/capacitor-speech-recognition（iOS SFSpeechRecognizer /
//                     Android SpeechRecognizer）
//   - 鸿蒙壳        → `fleetNative` 桥 → ArkTS Core Speech Kit（端侧，离线）
//
// **判定顺序是这个模块唯一真正要紧的东西**，因为 Web Speech 不能用 feature
// detection 来判：iOS WKWebView 里 Apple 关掉了识别功能，却仍然把
// `webkitSpeechRecognition` 挂在 window 上（WebKit #239816，挂了三年多没修）。
// 于是 `if (window.webkitSpeechRecognition)` 在 Capacitor 的 iOS 壳里为真，
// `start()` 却永远不出结果，也不报错 —— 表现为「按了没反应」的静默失败。
//
// 所以顺序必须是**先认壳、最后才回落 Web Speech**：壳的身份是确定的（原生桥对象
// 存在与否、Capacitor 运行时在不在），而 Web Speech 只在「确定不在任何壳里」时才
// 采信。反过来写就会在 iOS 壳里选中一条死路。
//
// 壳侧约定沿用既有的两条通道，没有新发明：web→壳走 `window.fleetNative.x()`
// （nativeScan.ts），壳→web 走具名 hook + pending 队列（nativePush.ts）。

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
   * 麦克风真的开始收音了。
   *
   * `start()` 返回 ≠ 已经在录：三条实现都要先跨一段异步 —— Web Speech 要等
   * 浏览器起识别会话，Capacitor 要查/要权限，鸿蒙要 `createEngine`。这段空窗里
   * 用户说的话**全丢**，而界面上它和「已经在录」长得一模一样，所以用户只会觉得
   * 「前半句没识别出来」。有了这一声，UI 才能在就绪之前老实说「准备中」。
   *
   * 每条实现都必须调，且只调一次。
   */
  onReady(): void;
  /** 说话过程中的临时结果，会被后续结果覆盖。用来做实时回显。 */
  onPartial(text: string): void;
  /** 定稿的一段文字。一次会话可能出多段（长语音被引擎自己切开）。 */
  onFinal(text: string): void;
  /** 出错。收到之后本次会话即结束，不会再有其它回调。 */
  onError(kind: VoiceErrorKind): void;
  /**
   * **引擎自己收工了** —— 不是调用方要求的。
   *
   * 三条实现都有这一刻，而且都不罕见：鸿蒙的 VAD 判定静默 3 秒（或录满 60 秒
   * 上限）、Web Speech 即便 continuous 也会在长静默后自行结束、Capacitor 那边
   * 是 `start()` 的 promise resolve。以前这三处都只在内部把会话标成死的，页面
   * 无从知道 —— 界面继续显示「正在听」，用户接着说却一个字都不出，只有再点一次
   * 停止才回得来。
   *
   * 与 onError 互斥，一次会话最多一个结局；调用方 cancel 之后不再上报。
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
