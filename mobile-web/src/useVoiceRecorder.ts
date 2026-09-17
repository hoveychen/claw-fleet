// State machine for the voice recorder bar — VoiceButton / VoiceBar handle rendering, this manages the full recording lifecycle.
//
// Division of labor with useVoiceInput below: that layer manages provider lifecycle (open mic, get results,
// report errors), this layer manages **what the user sees during one recording**: where text goes, how much
// was written, how long the recording took, whether they can still undo. The distinction shows in two things
// it must remember:
//
//   - `base`: the input field contents **before this recording started**. With it, "retry" is possible — otherwise
//     the user wanting to try again would have to manually delete the misrecognized segment from the input field.
//   - `preview`: finalized text + the floating-in portion, composed into one for direct display in the input field. Real-time
//     transcription lands in the actual input field (like Gboard / ChatGPT voice input), not crammed into a small line
//     that gets cut off.
//
// Voice output is text, not audio. So every decision at this layer biases toward "text can change, can undo anytime".

import { useCallback, useEffect, useRef, useState } from "react";
import { appendVoiceText, type VoiceErrorKind, type VoiceProviderId } from "./voiceInput";
import { useVoiceInput } from "./useVoiceInput";

/** Hold time ≤ this value counts as a "tap" — recording continues, finger can leave the screen. */
export const TAP_MS = 500;

/** What to do after pressing and releasing. */
export type PressIntent =
  /** Tap: enter (or stay in) recording mode; continues even if finger leaves. */
  | "keep"
  /** Long press: press-to-speak style; release to stop. */
  | "stop";

/**
 * Determine what to do on release. Only based on hold duration, no other dimensions.
 *
 * The old version had a "swipe-up to cancel" gesture: a blind gesture with no visible target, users had no way to know
 * how far to swipe (even WeChat shows visible "Cancel / Convert to text" targets when swiping). Cancel is now
 * a clear ✕ button on the recording bar.
 */
export function pressIntent(heldMs: number): PressIntent {
  return heldMs <= TAP_MS ? "keep" : "stop";
}

/** Format recording duration as `0:07` / `1:03`. */
export function formatDuration(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

/** How long after hearing the last content to still count as "speaking" — waveform uses this to decide whether to animate. */
export const SPEAKING_WINDOW_MS = 1200;

/**
 * Time to wait after "Stop and Send" is pressed for the final finalized segment.
 *
 * The engine **emits one more finalized segment after stop** (after finishing the last utterance). If sent immediately,
 * the user's last sentence is lost — and lost silently, after they've already put the phone down. So we pause
 * here to wait for it; each new finalized segment resets the timer, and we only actually send when it goes quiet.
 */
export const FINALIZE_MS = 1200;

export interface VoiceRecorderApi {
  /** Whether this device supports voice input. When false, caller should not render the voice input UI at all. */
  available: boolean;
  recording: boolean;
  /** Already started, but the engine hasn't yet confirmed "the mic is actually open". Anything spoken during this window
   *  won't reach any result, so the recorder bar can't pretend to be listening (no elapsed time, no waveform). */
  preparing: boolean;
  /** Whether the recorder bar should be visible on screen — there's a short wait after audio stops for the final finalized segment,
   *  during which the bar can't disappear yet (otherwise the user hits send and sees nothing happening for over a second). */
  active: boolean;
  /** Content the input field should display during recording (finalized + unfinalized combined). Equals the original value when not recording. */
  preview: string;
  /**
   * The input field should display `preview` right now, not `value`.
   *
   * **Not the same as `recording`**: after pressing stop, there's a short wait for the final finalized segment. That text
   * is still in `preview` but hasn't entered `value` yet. Switching back to `value` during this time makes the user see
   * "text disappeared" — exactly the symptom this version is fixing, just shorter.
   */
  showingPreview: boolean;
  /** The unfinalized portion, exposed separately so the caller can render it in gray. */
  partial: string;
  seconds: number;
  /** Heard new content within the last 1.2 seconds. */
  speaking: boolean;
  error: VoiceErrorKind | null;
  /** This recording session has already written something to the input field — determines whether to offer "retry". */
  dirty: boolean;
  /** The caller provided onSend, so the recorder bar has "Stop and Send". */
  canSend: boolean;
  /** Waiting for the final finalized segment before sending. */
  finalizing: boolean;
  start(): void;
  /** Stop listening, keep the result. */
  stop(): void;
  /** Discard this recording session: revert everything, including what's already in the input field. */
  cancel(): void;
  /** Revert to the state before this recording started, then restart. */
  retry(): void;
  /** Stop and send the content. Waits for the final finalized segment to arrive before actually sending. */
  stopAndSend(): void;
  /** Close the error message and return to ready state. */
  dismissError(): void;
  /** Whether this environment can take the user directly to open mic permissions (shell can, browser can't). */
  canOpenSettings: boolean;
  /** Launch the permission dialog; on success, automatically starts recording. */
  openSettings(): Promise<boolean>;
  /** For the current implementation, error messages use this to clarify "where are the permissions". */
  providerId: VoiceProviderId | null;
}

export function useVoiceRecorder({
  lang = "zh-CN",
  value,
  onChange,
  onSend,
}: {
  lang?: string;
  /** Current input field content. */
  value: string;
  /** Call this when voice should update the input field. */
  onChange: (next: string) => void;
  /** Only when provided does the recorder bar have "Stop and Send". When omitted, the bar only has stop (like decision card
   *  input fields, where send is on the card's own button). */
  onSend?: () => void;
}): VoiceRecorderApi {
  // Recognition results arrive asynchronously; meanwhile the user might type more. A captured `value` in the closure
  // is stale, and concatenating with it overwrites the new characters. Both are read from refs holding the latest render values.
  const valueRef = useRef(value);
  valueRef.current = value;
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;
  // Send happens in a timer callback, after the component has re-rendered several times; using a captured closure
  // carries stale input field content to send — which silently loses the final finalized segment we just waited for.
  const onSendRef = useRef(onSend);
  onSendRef.current = onSend;

  /** Input field contents before this recording started. Cancel / retry both revert to it. */
  const baseRef = useRef<string | null>(null);
  const [dirty, setDirty] = useState(false);

  /** Timer waiting after "Stop and Send" is pressed. null = not waiting. */
  const finalizeTimerRef = useRef<number | null>(null);
  const [finalizing, setFinalizing] = useState(false);

  const clearFinalize = useCallback(() => {
    if (finalizeTimerRef.current !== null) {
      window.clearTimeout(finalizeTimerRef.current);
      finalizeTimerRef.current = null;
    }
    setFinalizing(false);
  }, []);

  /** Reset the "how long has it been quiet" counter. Each new finalized segment restarts it; only send when truly quiet. */
  const armFinalize = useCallback(() => {
    if (finalizeTimerRef.current !== null) window.clearTimeout(finalizeTimerRef.current);
    setFinalizing(true);
    finalizeTimerRef.current = window.setTimeout(() => {
      finalizeTimerRef.current = null;
      setFinalizing(false);
      onSendRef.current?.();
    }, FINALIZE_MS);
  }, []);

  const voice = useVoiceInput(lang, (text) => {
    setDirty(true);
    onChangeRef.current(appendVoiceText(valueRef.current, text));
    // Already waiting to send — this newly arrived finalized segment means the engine isn't done yet, restart the timer.
    if (finalizeTimerRef.current !== null) armFinalize();
  });

  const recording = voice.state === "listening";
  // Audio stopped but still waiting for the final finalized segment. The input field should keep showing that text
  // during this time (see voiceTail.ts), so it and "recording" together decide whether to show preview or value.
  const settling = voice.settling;
  const preparing = voice.state === "preparing";

  // Elapsed time counter. Only runs while recording, resets on stop — it's a property of "this recording".
  const [seconds, setSeconds] = useState(0);
  useEffect(() => {
    if (!recording) {
      setSeconds(0);
      return;
    }
    const started = Date.now();
    const id = window.setInterval(() => {
      setSeconds((Date.now() - started) / 1000);
    }, 250);
    return () => window.clearInterval(id);
  }, [recording]);

  // "Speaking": inferred from when recognition results arrive. Real audio volume is unavailable — Web Speech API doesn't expose it,
  // and the in-app engine holds the mic; opening another getUserMedia for volume would risk mic contention.
  // So the waveform is an **activity indicator**, not a volume meter, anchored to a real signal: the engine is actually producing text.
  const [speaking, setSpeaking] = useState(false);
  const heardAtRef = useRef(0);
  useEffect(() => {
    if (voice.partial) heardAtRef.current = Date.now();
  }, [voice.partial]);
  useEffect(() => {
    if (!recording) {
      setSpeaking(false);
      return;
    }
    const id = window.setInterval(() => {
      setSpeaking(Date.now() - heardAtRef.current < SPEAKING_WINDOW_MS);
    }, 200);
    return () => window.clearInterval(id);
  }, [recording]);

  const start = useCallback(() => {
    clearFinalize();
    if (baseRef.current === null) {
      baseRef.current = valueRef.current;
      setDirty(false);
    }
    voice.start();
  }, [voice, clearFinalize]);

  const stop = useCallback(() => {
    baseRef.current = null;
    voice.stop();
  }, [voice]);

  const stopAndSend = useCallback(() => {
    baseRef.current = null;
    voice.stop();
    armFinalize();
  }, [voice, armFinalize]);

  // Don't leave a pending send timer around after the component unmounts — it would send to an already-unmounted input field.
  useEffect(() => clearFinalize, [clearFinalize]);

  const cancel = useCallback(() => {
    clearFinalize();
    // Cancel must also revert **the finalized text already written to the input field**. The engine breaks long speech into
    // multiple segments and finalizes them progressively; if we only stop listening, sentences that arrived before the user
    // pressed ✕ stay in the field — that's not "cancel", that's "stop".
    const base = baseRef.current;
    baseRef.current = null;
    voice.cancel();
    if (base !== null && base !== valueRef.current) onChangeRef.current(base);
    setDirty(false);
  }, [voice, clearFinalize]);

  const retry = useCallback(() => {
    clearFinalize();
    const base = baseRef.current ?? valueRef.current;
    voice.cancel();
    if (base !== valueRef.current) onChangeRef.current(base);
    setDirty(false);
    baseRef.current = base;
    voice.start();
  }, [voice, clearFinalize]);

  const dismissError = useCallback(() => {
    baseRef.current = null;
    voice.clearError();
  }, [voice]);

  // On successful permission grant, immediately start recording. Making the user get permission and then come back to tap the mic again
  // splits one action into two — but what they just did was "I want to talk".
  const grantAndStart = useCallback(async (): Promise<boolean> => {
    const granted = await voice.openSettings();
    if (granted) start();
    return granted;
  }, [voice, start]);

  return {
    available: voice.state !== "probing" && voice.state !== "unsupported",
    recording,
    preparing,
    active: recording || preparing || finalizing || settling,
    showingPreview: recording || settling,
    preview: voice.partial ? appendVoiceText(value, voice.partial) : value,
    partial: voice.partial,
    seconds,
    speaking,
    error: voice.state === "error" ? voice.error : null,
    dirty,
    canSend: !!onSend,
    finalizing,
    start,
    stop,
    stopAndSend,
    cancel,
    retry,
    dismissError,
    canOpenSettings: voice.canOpenSettings,
    openSettings: grantAndStart,
    providerId: voice.providerId,
  };
}

/**
 * Keep the input field scrolled to the latest text while recording.
 *
 * A read-only textarea won't scroll itself. For longer recordings, the user sees the field stuck at the beginning and assumes
 * it's not listening — but real-time transcription is the main evidence in this redesign that "is it hearing me?", and if they
 * can't see it, they think it's not.
 */
export function useFollowTail<T extends HTMLElement>(
  active: boolean,
  content: unknown,
): React.RefObject<T | null> {
  const ref = useRef<T>(null);
  useEffect(() => {
    const el = ref.current;
    if (el && active) el.scrollTop = el.scrollHeight;
  }, [active, content]);
  return ref;
}
