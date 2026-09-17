// Voice recognition in the Capacitor shell (iOS / Android).
//
// Underneath is iOS's SFSpeechRecognizer and Android's SpeechRecognizer, bridged by
// @capgo/capacitor-speech-recognition. We chose this package over writing our own
// Capacitor plugin: rolling our own requires maintaining Swift + Kotlin, while this
// stays actively maintained, supports punctuation and segmented sessions, and we already
// use its sibling @capgo/capacitor-share-target.
//
// **On Android, this relies on GMS**: on Chinese ROMs without Google services,
// `SpeechRecognizer.isRecognitionAvailable()` returns false, and the plugin's available()
// honestly returns false, so the button doesn't appear. This is the deliberate scope
// boundary (no cloud-service fallback), not a bug — see the voice-input-native plan's
// trade-offs.

import { SpeechRecognition } from "@capgo/capacitor-speech-recognition";
import type {
  VoiceErrorKind,
  VoiceHandlers,
  VoiceInputProvider,
  VoiceSession,
} from "./voiceInput";

/**
 * Native error code → our classification.
 *
 * **The plugin doesn't document an error code enum**: `SpeechRecognitionErrorEvent.code`
 * is a pass-through string from the native side (Android's SpeechRecognizer constants,
 * iOS's NSError), and the two platforms speak different languages. So we keyword-match
 * here and fall back to unavailable for unknowns — it's better to be vague than misattribute
 * a network issue as a permission failure.
 *
 * On real device acceptance (P6) this table will be calibrated against actual error
 * codes.
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
      // Plugin not installed / native side threw — treat as unavailable, button doesn't appear.
      return false;
    }
  },

  async start(lang: string, handlers: VoiceHandlers): Promise<VoiceSession> {
    // After cancel, native side still sends events. Caller said no; swallow all
    // subsequent ones.
    let dead = false;
    const listeners: { remove: () => void }[] = [];
    const teardown = () => {
      for (const l of listeners) l.remove();
      listeners.length = 0;
    };

    // Permissions: check first, then request. On iOS this covers both speech
    // recognition and microphone authorization.
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

    // Ready only when the native side actually starts capturing. 'startingListening'
    // doesn't count — that's still spinning up the recognition session and speech is
    // still lost.
    listeners.push(
      await SpeechRecognition.addListener("listeningState", (e) => {
        if (dead) return;
        if (e.state === "started") handlers.onReady();
      }),
    );

    listeners.push(
      await SpeechRecognition.addListener("partialResults", (e) => {
        if (dead) return;
        // In continuous PTT sessions, accumulatedText is the full text including this
        // round, more complete than matches[0]; fall back to this round's first match
        // if it's absent.
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

    // **Don't await this promise**: the plugin's start() doesn't resolve until the
    // entire recognition ends. Awaiting it would hold VoiceSession hostage until the
    // user finishes speaking, leaving the UI with no handle to press stop. Fire it, hang
    // the cleanup, and hand the session out immediately.
    void SpeechRecognition.start({
      language: lang,
      partialResults: true,
      // iOS 16+ native punctuation. Speech input goes straight to the prompt, and it's
      // hard to read without punctuation.
      addPunctuation: true,
    })
      .then(({ matches }) => {
        if (dead) return;
        dead = true;
        teardown();
        const text = matches?.[0];
        if (text) handlers.onFinal(text);
        // Promise resolution means "this recognition is done", whether the user pressed
        // stop or the native side decided speech is over. The caller uses this to close
        // "listening".
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
      // stop lets the native side wrap up normally; the .then above gets the final result.
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
