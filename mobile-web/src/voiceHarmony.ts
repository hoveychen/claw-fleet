// HarmonyOS shell speech recognition. Web Speech API isn't available in ArkWeb,
// so capabilities come from the shell: web calls `fleetNative.startVoice()`, and
// results push back via `window.__fleetVoice` (shell side: mobile-harmony's
// WebShell.ets + common/SpeechAsr.ets).
//
// This uses Core Speech Kit's on-device model: offline, free, no quota—the only
// provider of the three that doesn't need the network. The trade-off: **Chinese only**.
// Official docs explicitly limit support to Chinese, so English identifiers in
// mixed-language text like "merge a worktree" will be weak spots. This is a hard
// boundary of this route, not something web-side code can fix.
//
// Unlike __fleetShare and __fleetPushToken, we don't need a pending queue here:
// those two arrive at cold startup before React effects; voice events only fire
// after the user presses the button, by which time the hook is already registered.

import type {
  VoiceErrorKind,
  VoiceHandlers,
  VoiceInputProvider,
  VoiceSession,
} from "./voiceInput";

/** Name of the shell-injected bridge object; same as in nativeScan.ts. */
const BRIDGE = "fleetNative";
/** Entry point where the shell pushes recognition events back to the page. */
const HOOK = "__fleetVoice";
/** Entry point where the shell pushes the permission re-grant result back to the page. */
const PERM_HOOK = "__fleetVoicePermission";

/** How long to wait for the re-grant panel to respond before giving up.
 *  Users may browse the panel for a while, so give it plenty of time. */
const PERM_TIMEOUT_MS = 120_000;

/** Shape of VoiceEvent from the shell side (WebShell.ets). */
interface HarmonyVoiceEvent {
  kind: "ready" | "partial" | "final" | "error" | "end";
  text: string;
  code: string;
}

interface HarmonyBridge {
  startVoice?: (lang: string) => void;
  stopVoice?: () => void;
  cancelVoice?: () => void;
  openVoiceSettings?: () => void;
}

function bridge(): HarmonyBridge | undefined {
  return (window as unknown as Record<string, HarmonyBridge | undefined>)[BRIDGE];
}

/**
 * Shell-side error code → our classification.
 *
 * `PERMISSION_DENIED` and `START_FAILED` are strings defined by SpeechAsr.ets itself;
 * the rest are Core Speech Kit's numeric error codes converted to strings. Since the
 * numeric codes have no public enum, we conservatively bucket them all as unavailable—
 * same as the Capacitor approach: better to be vague than to call an engine-internal
 * error "no permission" and send the user on a wild goose chase to settings.
 */
export function classifyHarmonyError(code: string): VoiceErrorKind {
  if (code === "PERMISSION_DENIED") return "no-permission";
  return "unavailable";
}

export const harmonyVoiceProvider: VoiceInputProvider = {
  id: "harmony",

  // startVoice on the bridge means it's available. The shell-side engine is a
  // built-in on-device model; there's no "is this device's recognition service
  // installed?" question, so we don't need to ask the native side again.
  async isAvailable(): Promise<boolean> {
    return typeof bridge()?.startVoice === "function";
  },

  // Once a user denies permission, HarmonyOS's requestPermissionsFromUser never
  // pops again—so the "Please allow in system settings" text on screen becomes a
  // dead end: the user doesn't know where to go, and we have no way to send them.
  // The shell side's requestPermissionOnSetting is the official re-grant entry—
  // it pops the system panel directly inside the app.
  async openPermissionSettings(): Promise<boolean> {
    const b = bridge();
    if (typeof b?.openVoiceSettings !== "function") return false;

    const w = window as unknown as Record<string, unknown>;
    return new Promise<boolean>((resolve) => {
      let done = false;
      const finish = (granted: boolean) => {
        if (done) return;
        done = true;
        delete w[PERM_HOOK];
        resolve(granted);
      };
      w[PERM_HOOK] = (granted: boolean) => finish(granted === true);
      // If the panel doesn't respond, we can't hang forever (on some devices,
      // the re-grant panel doesn't pop). Fall through as denied; the user still
      // sees an error block they can click again, not a frozen UI.
      setTimeout(() => finish(false), PERM_TIMEOUT_MS);
      b.openVoiceSettings?.();
    });
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
          // Engine finished on its own (VAD detected speech end, or hit maxAudioDuration).
          // Finalization already came through "final" earlier. Here we tear down the hook
          // **and tell the caller the session is over**—skip the second part and the page
          // stays stuck on "Listening", even though the user can keep talking and gets
          // no text.
          dead = true;
          teardown();
          handlers.onEnd();
          break;
      }
    };

    b.startVoice(lang);

    return {
      // stop lets the engine finalize normally; the last segment still arrives via "final",
      // so we don't tear down the hook here—tear it down and that segment has no handler,
      // showing up as "I pressed Stop after speaking, but the last sentence didn't appear".
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
