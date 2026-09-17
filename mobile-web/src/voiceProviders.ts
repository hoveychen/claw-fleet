// Provider registry: maps environments detected by detectVoiceProvider() to concrete implementations.
//
// A separate module so useVoiceInput doesn't have to import three implementations — that would drag Capacitor
// plugins and Harmony bridge code into the browser bundle. Here we only touch the chosen implementation.
//
// Currently only Web Speech is registered; Capacitor and Harmony are wired in their own P-tasks, at which point
// just add a line to REGISTRY — callers need no changes.

import { detectVoiceProvider, type VoiceInputProvider, type VoiceProviderId } from "./voiceInput";
import { capacitorVoiceProvider } from "./voiceCapacitor";
import { harmonyVoiceProvider } from "./voiceHarmony";
import { webSpeechProvider } from "./voiceWebSpeech";

const REGISTRY: Partial<Record<VoiceProviderId, VoiceInputProvider>> = {
  "web-speech": webSpeechProvider,
  capacitor: capacitorVoiceProvider,
  harmony: harmonyVoiceProvider,
};

/** The provider for the current environment, or null if no suitable implementation is available. */
export function currentVoiceProvider(): VoiceInputProvider | null {
  const id = detectVoiceProvider();
  return id ? (REGISTRY[id] ?? null) : null;
}
