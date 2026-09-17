// Browser / PWA speech recognition using the Web Speech API.
//
// Only serves environments "definitely not in any native shell" — detection
// belongs to voiceInput.ts::detectVoiceProvider, we don't re-detect here
// (webkitSpeechRecognition lies in shell; see that file's header).
//
// This path sends audio to **vendor servers** for recognition: Chrome → Google,
// Safari → Apple. So on domestic Android Chrome it typically fails with network
// error — not a bug, it's an inherent boundary of this implementation. True
// offline recognition is in the other two providers.

import type {
  VoiceErrorKind,
  VoiceHandlers,
  VoiceInputProvider,
  VoiceSession,
} from "./voiceInput";
import { hasWebSpeech } from "./voiceInput";

/** Web Speech constructor, non-prefixed version first. */
function ctor(): (new () => SpeechRecognitionLike) | undefined {
  const w = window as unknown as Record<string, unknown>;
  const c = w["SpeechRecognition"] ?? w["webkitSpeechRecognition"];
  return typeof c === "function" ? (c as new () => SpeechRecognitionLike) : undefined;
}

/** The SpeechRecognition parts we use. lib.dom doesn't have this type everywhere,
 *  so we define it ourselves. */
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
 * Spec error strings → our categories.
 *
 * Both `service-not-allowed` and `not-allowed` map to permission: the former
 * is system/browser policy rejection (Chrome on iOS always reports this), the
 * latter is user denying the mic. To the user both mean "go enable permission",
 * so separating them is meaningless.
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

  // Constructor existing means available. We can't probe "is vendor server
  // reachable" here — that requires actually opening the mic, cost too high,
  // let start()'s network error report it.
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
    // continuous: don't auto-stop after one sentence, let user press stop.
    // Voice input is often multiple sentences; the engine's default "one
    // sentence then done" would eat the rest.
    rec.continuous = true;
    rec.interimResults = true;

    // After cancel, engine still fires onend (some implementations even fire
    // aborted onerror once). Caller said no results wanted, so swallow
    // everything after this, don't report it as an error.
    let dead = false;

    // Spec's onstart means "audio capture started", exactly what we need.
    rec.onstart = () => {
      if (dead) return;
      handlers.onReady();
    };

    rec.onresult = (e) => {
      if (dead) return;
      // Only look at new results from this event: results array is cumulative;
      // iterating from start would re-report already-finalized segments.
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

    // Browsers end sessions after long silence even with continuous=true
    // (varies by implementation). We also reach here after our own stop() —
    // upper layers already returned to idle, getting one more end notice is
    // harmless (it decides what to do by state). On cancel/error paths, dead
    // is already true, won't re-report.
    rec.onend = () => {
      if (dead) return;
      dead = true;
      handlers.onEnd();
    };

    rec.start();

    return {
      // stop: stop recording but let engine finalize and emit the last segment
      // (fires onresult once more).
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
