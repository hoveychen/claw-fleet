// Voice input state machine. UI (VoiceButton) only renders; recognition lifecycle lives here.
//
// Provider is chosen by currentVoiceProvider(); this hook is environment-agnostic.
// Three implementations below are equivalent at this layer, so P4/P5 integrations
// with Capacitor and Harmony won't need changes to this file.

import { useCallback, useEffect, useRef, useState } from "react";
import { t } from "./i18n";
import type { VoiceErrorKind, VoiceProviderId, VoiceSession } from "./voiceInput";
import { currentVoiceProvider } from "./voiceProviders";
import { createTailGuard, type TailGuard } from "./voiceTail";
import { holdWakeLock } from "./wakeLock";

export type VoiceState =
  /** Still probing the provider for availability. Button is not shown to avoid flicker. */
  | "probing"
  /** No available recognition service in this environment. Button doesn't appear. */
  | "unsupported"
  | "idle"
  /**
   * Recognition started, but the microphone hasn't actually begun capturing audio yet.
   *
   * All three implementations must first cross an async boundary (starting recognition
   * session / checking permissions / createEngine). During this gap, user input is lost.
   * Previously this gap was displayed as "Listening", which confused users ("Why didn't
   * the first half get recognized?"). Breaking it into its own state lets the UI
   * honestly say "Preparing".
   */
  | "preparing"
  | "listening"
  | "error";

/** Error text shown to users. No need for fine-grained distinctions — users only have a few actions anyway. */
export function voiceErrorText(kind: VoiceErrorKind): string {
  switch (kind) {
    case "no-permission":
      // "Please allow in system settings" was moved to voiceErrorHint: environments
      // that can send users there don't actually need this text (the button is the action),
      // while environments that can't need to clearly say which *specific* settings —
      // browsers and shells have completely different places.
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
 * Second line of error hint: **what the user can do right now**.
 *
 * Why environment-specific: "Please allow the microphone in system settings" is wrong
 * in browsers (that's site permission, not system settings), and redundant in shells
 * that can directly launch the permission panel (there's a button right there). A hint
 * pointing the wrong way is worse than no hint — users really will dig through settings
 * and come back finding nothing worked.
 *
 * @param canOpenSettings Whether this environment can directly send users to the permission grant
 *   (shells can; browsers cannot).
 * @param providerId The current implementation; determines how to phrase "where permissions are".
 */
export function voiceErrorHint(
  kind: VoiceErrorKind,
  canOpenSettings: boolean,
  providerId: VoiceProviderId | null,
): string | null {
  if (kind === "no-permission") {
    // If there's a button, don't also write text—the button itself is the instruction.
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
 * Whether this device can do voice input.
 *
 * Extracted separately because **two places must use the same logic**: the voice button
 * (not shown if unavailable) and the input field placeholder ("Send a message or press
 * and hold to talk"). If we checked differently in each place, devices without GMS
 * would show "You can press and hold to talk" with no button present — a hint pointing
 * to nonexistent UI is worse than no hint.
 *
 * Async because the native side must check permissions and services: Android's
 * isRecognitionAvailable() returns false on ROMs without Google services, so we only
 * know by asking.
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
  /** Real-time interim result from speech recognition, not yet final. */
  partial: string;
  /** The reason when state is "error". */
  error: VoiceErrorKind | null;
  /** Audio has stopped, waiting for the engine to finalize the last segment. partial still displays. */
  settling: boolean;
  start(): void;
  /** Stop recording and keep the recognized content. */
  stop(): void;
  /** Discard this recognition attempt. */
  cancel(): void;
  /** Clear the error and return to idle. cancel() doesn't do this: it only stops the
   *  microphone; the error should stay on screen until the user has seen it. */
  clearError(): void;
  /** Whether this environment can send the user to grant microphone permission.
   *  When false, the UI should only show explanatory text, not a button that does nothing. */
  canOpenSettings: boolean;
  /** Open the permission grant interface; returns whether the user granted it.
   *  Always false when canOpenSettings is false. */
  openSettings(): Promise<boolean>;
  /** Which implementation is currently active. Error messages use this to clarify "where permissions are". */
  providerId: VoiceProviderId | null;
}

/**
 * @param lang Recognition language, e.g., `zh-CN`.
 * @param onText Finalized text segment. A single recognition may trigger multiple callbacks (engine does its own segmentation).
 */
export function useVoiceInput(lang: string, onText: (text: string) => void): UseVoiceInput {
  const [state, setState] = useState<VoiceState>("probing");
  const [partial, setPartial] = useState("");
  const [error, setError] = useState<VoiceErrorKind | null>(null);
  /** Stopped but still waiting for the last segment to finalize. That text must stay on screen; see voiceTail.ts. */
  const [settling, setSettling] = useState(false);
  const sessionRef = useRef<VoiceSession | null>(null);

  // onText is a fresh function on each render (callers usually write inline arrows).
  // Storing in a ref means start() callbacks don't need to list it as a dependency,
  // and a parent re-render won't invalidate an in-progress recognition.
  const onTextRef = useRef(onText);
  onTextRef.current = onText;

  // Probe availability once on mount.
  const probe = useVoiceAvailable();
  useEffect(() => {
    if (probe === "probing") return;
    setState((s) => (s === "probing" ? (probe === "ready" ? "idle" : "unsupported") : s));
  }, [probe]);

  // Force the screen to stay on during recording, regardless of the user's screen-on preference.
  //
  // Phones default to turning off after ~30 seconds of no touch, but hands don't touch
  // the screen while talking — a longer utterance will definitely trigger it. When the
  // screen goes off, the WebView is suspended by the OS, and the recognition session
  // stops immediately. From the user's perspective, the app looks killed, everything
  // they just said is gone, and there's no explanation. This isn't a "nice to have"—
  // it's critical to whether this flow works at all.
  //
  // "preparing" counts too: that gap is when the user has started speaking but the engine
  // hasn't opened the mic yet—screen off is just as fatal. Release timing follows state;
  // error, stop, or unmount will all trigger cleanup.
  useEffect(() => {
    if (state !== "listening" && state !== "preparing") return;
    return holdWakeLock();
  }, [state]);

  // Fallback for text that hasn't finalized when recording stops; see voiceTail.ts.
  // Finalized text from this fallback and from the engine use the same callback —
  // the caller can't (and shouldn't) tell them apart.
  const tailRef = useRef<TailGuard | null>(null);
  if (tailRef.current === null) {
    tailRef.current = createTailGuard((text) => onTextRef.current(text), {
      onSettle: () => {
        setSettling(false);
        setPartial("");
      },
    });
  }
  const tail = tailRef.current;

  // Clean up any in-progress recognition on unmount, or the microphone stays open.
  useEffect(() => {
    return () => {
      sessionRef.current?.cancel();
      sessionRef.current = null;
      tail.dispose();
    };
  }, [tail]);

  // Bump a generation number on each start / stop / cancel. provider.start() is async,
  // so by the time it resolves, the user may have already cancelled. Comparing generations
  // tells us whether the result still counts.
  //
  // We can't just check state: setState is async, so in the frame right after cancel,
  // before the state update lands, state is still "listening". A guard that only checks
  // state would miss the cancel, leaving the microphone open.
  const genRef = useRef(0);

  const start = useCallback(() => {
    const provider = currentVoiceProvider();
    if (!provider || sessionRef.current) return;
    const gen = ++genRef.current;
    // A new round cancels any tail finalization still pending from the last round:
    // that text belongs to the previous recording, which the user has abandoned.
    tail.cancel();
    setSettling(false);
    setError(null);
    setPartial("");
    setState("preparing");
    void provider
      .start(lang, {
        onReady: () => {
          if (gen !== genRef.current) return;
          setState((s) => (s === "preparing" ? "listening" : s));
        },
        // All three callbacks check the generation first. The web-speech provider
        // goes silent after cancel on its own, but that's an implementation detail;
        // this layer shouldn't rely on every provider being equally careful—miss one
        // and you get "text keeps appearing in the input after cancel". Text arriving
        // means recording has started, so if some implementation forgot onReady, this
        // catches it and prevents the UI from getting stuck on "Preparing".
        onPartial: (text) => {
          if (gen !== genRef.current) return;
          setState((s) => (s === "preparing" ? "listening" : s));
          setPartial(text);
          tail.partial(text);
        },
        onFinal: (text) => {
          if (gen !== genRef.current) return;
          setState((s) => (s === "preparing" ? "listening" : s));
          setPartial("");
          // Finalization takes over this segment; discard any pending tail text,
          // or the same utterance arrives twice.
          tail.final();
          onTextRef.current(text);
        },
        // The engine finished on its own (HarmonyOS VAD detects 3s silence,
        // Web Speech auto-ends, native recognition completes). This isn't an error
        // or a user action—the UI should wind down as if Stop was pressed, not keep
        // pretending to listen. Any unfinal text follows the same finalization path as Stop.
        onEnd: () => {
          if (gen !== genRef.current) return;
          // Session handle not yet acquired, or we already stopped it—this callback
          // is an echo and shouldn't trigger cleanup again.
          if (!sessionRef.current) return;
          sessionRef.current = null;
          const awaiting = tail.stop();
          setSettling(awaiting);
          if (!awaiting) setPartial("");
          setState((s) => (s === "listening" || s === "preparing" ? "idle" : s));
        },
        onError: (kind) => {
          if (gen !== genRef.current) return;
          sessionRef.current = null;
          setPartial("");
          tail.cancel();
          setSettling(false);
          // User-initiated cancel isn't an error; silently return to idle.
          if (kind === "aborted") {
            setState("idle");
            return;
          }
          setError(kind);
          setState("error");
        },
      })
      .then((session) => {
        // By the time this resolves, the user may have already cancelled.
        // If so, immediately clean up the session, or the microphone stays open
        // while the UI has already returned to idle—completely invisible to the user.
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
  }, [lang, tail]);

  // Note: stop does NOT bump the generation: the engine still needs to finalize
  // the last segment after stop, and if we bumped it, that finalization would be
  // dropped—showing up as "I pressed Stop after speaking, but the last sentence
  // didn't appear". Only cancel (discard) and start (new round invalidates old) bump it.
  const stop = useCallback(() => {
    sessionRef.current?.stop();
    sessionRef.current = null;
    // Snapshot any unfinal text from the screen: if the engine provides finalization,
    // use that; otherwise finalize it when the timeout fires. Without this, whether
    // text appears depends on race conditions between the two callbacks.
    //
    // Critically, do NOT clear partial here: the text must stay in the input field
    // while waiting, or the user sees "text disappears, then reappears". Clearing
    // happens in onSettle.
    const awaiting = tail.stop();
    setSettling(awaiting);
    if (!awaiting) setPartial("");
    setState((s) => (s === "listening" || s === "preparing" ? "idle" : s));
  }, [tail]);

  const cancel = useCallback(() => {
    genRef.current++;
    sessionRef.current?.cancel();
    sessionRef.current = null;
    setPartial("");
    tail.cancel();
    setSettling(false);
    setState((s) => (s === "listening" || s === "preparing" ? "idle" : s));
  }, [tail]);

  const clearError = useCallback(() => {
    setError(null);
    setState((s) => (s === "error" ? "idle" : s));
  }, []);

  // Only native shells can implement this (browsers have no API to open site permission settings).
  // Detection happens at render time, not in state: provider is fetched synchronously,
  // and this value only controls whether the error block shows a button or just text.
  const provider = currentVoiceProvider();
  const canOpenSettings = typeof provider?.openPermissionSettings === "function";
  const providerId = provider?.id ?? null;

  const openSettings = useCallback(async (): Promise<boolean> => {
    const provider = currentVoiceProvider();
    if (typeof provider?.openPermissionSettings !== "function") return false;
    const granted = await provider.openPermissionSettings();
    // If permission was granted, clear the error—that message no longer applies,
    // and leaving it would make the user think the grant didn't work.
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
    settling,
    start,
    stop,
    cancel,
    clearError,
    canOpenSettings,
    openSettings,
    providerId,
  };
}
