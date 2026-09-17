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

/** Which implementation serves the current environment. */
export type VoiceProviderId = "web-speech" | "capacitor" | "harmony";

/** Reason for recognition failure. UI only needs to distinguish these types to decide what to say. */
export type VoiceErrorKind =
  /** User denied microphone / speech recognition permission. Can guide them to open it in settings. */
  | "no-permission"
  /** No speech detected throughout. Normal case, silent close-out is OK. */
  | "no-speech"
  /** Cannot connect to recognition service. Web Speech is the norm in China (audio is sent to vendor servers). */
  | "network"
  /** This device has no available recognition service at all, e.g. domestic Android phones without GMS. */
  | "unavailable"
  /** Caller canceled it themselves. */
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

/** One ongoing recognition session. */
export interface VoiceSession {
  /** Stop recording, finalize what was already heard (will trigger onFinal once more). */
  stop(): void;
  /** Discard this session's results, no more callbacks. */
  cancel(): void;
}

export interface VoiceInputProvider {
  readonly id: VoiceProviderId;
  /**
   * Whether this device can actually recognize right now. Async because the native side
   * needs to check permissions and service availability — on domestic Android phones without
   * GMS, Android's `isRecognitionAvailable()` returns false, only asking tells for sure.
   */
  isAvailable(): Promise<boolean>;
  start(lang: string, handlers: VoiceHandlers): Promise<VoiceSession>;
  /**
   * Guide the user to a place where they can open microphone permission, and report
   * whether authorization is now granted.
   *
   * Optional because **only native shells can do it**: there is no API in browsers to open
   * site permission settings, that path can only give an instruction string. So there is no
   * "all providers implement an empty shell" version here — absence means absence, the UI decides
   * based on this whether to draw a real button or just text, and never draws an unclickable button.
   *
   * HarmonyOS implementation uses `requestPermissionOnSetting`: after the user rejects once,
   * `requestPermissionsFromUser` never prompts again, this API is the official second-time
   * authorization entry, and it pops the system panel directly in-app, shorter than jumping
   * to settings and making the user find it themselves.
   */
  openPermissionSettings?(): Promise<boolean>;
}

/** Native bridge object name injected by HarmonyOS shell, same as in nativeScan.ts. */
const BRIDGE = "fleetNative";

/** Method name for voice on the bridge. Shell side's methodList must register the same-named method for it to count. */
const BRIDGE_START = "startVoice";

/**
 * Which implementation to use in the current environment; returns null if unclear
 * (e.g. Firefox on desktop browser without Web Speech).
 *
 * Note: this checks **environment identity**, not Web API existence:
 *   - HarmonyOS: whether the bridge has startVoice registered. When the shell has no voice
 *     support, this is false and we continue down — so old shell + new web never picks a
 *     nonexistent implementation.
 *   - Capacitor: Capacitor runtime reports itself on a native platform. Whether the plugin
 *     is installed is a separate question (provider's own isAvailable answers it), but
 *     "we're in a shell" is definite.
 *   - Otherwise: browser / PWA, then webkitSpeechRecognition existence is trustworthy.
 */
export function detectVoiceProvider(): VoiceProviderId | null {
  const w = window as unknown as Record<string, Record<string, unknown> | undefined>;
  if (typeof w[BRIDGE]?.[BRIDGE_START] === "function") return "harmony";
  if (Capacitor.isNativePlatform()) return "capacitor";
  if (hasWebSpeech()) return "web-speech";
  return null;
}

/**
 * Append recognized text to existing input field content.
 *
 * Whether to add a space depends on what's on both sides of the seam: adding space between
 * Chinese is wrong, and two English words stuck together is also wrong. Fleet's voice content
 * is naturally mixed Chinese/English ("把 P3 勾掉", "合一下 worktree"), so both cases appear
 * in the same utterance — only add space when both sides of the seam are ASCII letters/digits.
 */
export function appendVoiceText(existing: string, addition: string): string {
  const add = addition.trim();
  if (!add) return existing;
  if (!existing) return add;
  const left = existing[existing.length - 1];
  // Existing content already ends with whitespace; adding another would be a double space.
  if (/\s/.test(left)) return existing + add;
  const wordish = /[A-Za-z0-9]/;
  return wordish.test(left) && wordish.test(add[0]) ? `${existing} ${add}` : existing + add;
}

/**
 * Does the browser have the recognition part of Web Speech.
 *
 * **Only call this after confirming we are not in any native shell** — see file header, it lies in the shell.
 */
export function hasWebSpeech(): boolean {
  const w = window as unknown as Record<string, unknown>;
  return (
    typeof w["SpeechRecognition"] === "function" ||
    typeof w["webkitSpeechRecognition"] === "function"
  );
}
