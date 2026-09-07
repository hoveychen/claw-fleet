import { useEffect, useRef } from "react";
import { listen } from "@tauri-apps/api/event";
import type { WaitingAlert } from "../types";
import { useDecisionStore } from "../store";
import { getItem } from "../storage";
import { playAlertSound, type TtsMode } from "../audio";

/**
 * Chime / TTS when a session starts waiting for input.
 *
 * Claude Code sessions MAY route their wait-for-input through the
 * AskUserQuestion → DecisionPanel bridge, which owns the audio cue there.
 * When that bridge fires we must not double-announce. But plain waitingInput
 * (no AskUserQuestion) for claude-code still needs a sound — otherwise Boss
 * gets silently-pending sessions. Strategy: defer the claude-code chime
 * slightly; if a matching decision arrives within the delay, cancel
 * (DecisionPanel will cover it); otherwise play.
 *
 * This used to live inside the bottom-right `WaitingAlerts` card stack. The
 * cards were dropped (2026-09-06) for being low-signal; the sound was kept, so
 * the listener moved here as a headless hook mounted at the App root.
 */
export function useWaitingAlertSound() {
  const spokenIds = useRef(new Set<string>());

  useEffect(() => {
    const unlistenPromise = listen<WaitingAlert[]>("waiting-alerts-updated", (e) => {
      const ttsMode = (getItem("tts-mode") as TtsMode) || "chime_and_speech";
      if (ttsMode === "off") return;
      for (const alert of e.payload) {
        if (spokenIds.current.has(alert.sessionId)) continue;
        spokenIds.current.add(alert.sessionId);
        if (alert.source === "claude-code") {
          const { sessionId, summary } = alert;
          setTimeout(() => {
            const decisions = useDecisionStore.getState().decisions;
            const handledByPanel = decisions.some(
              (d) => d.request?.sessionId === sessionId,
            );
            if (!handledByPanel) playAlertSound(summary);
          }, 800);
        } else {
          playAlertSound(alert.summary);
        }
      }
    });

    return () => {
      unlistenPromise.then((u) => u());
    };
  }, []);
}
