/**
 * Notification chime presets and TTS helpers.
 *
 * All chimes are synthesised with the Web Audio API — no audio files needed.
 */

import i18n from "i18next";
import { getItem } from "./storage";

// ── Chime presets ────────────────────────────────────────────────────────────

export type ChimePreset = "ding_dong" | "soft_bell" | "triple" | "drop";

export const CHIME_PRESETS: ChimePreset[] = ["ding_dong", "soft_bell", "triple", "drop"];

/** Duration (ms) of each chime — used to schedule TTS after chime. */
const CHIME_DURATIONS: Record<ChimePreset, number> = {
  ding_dong: 600,
  soft_bell: 500,
  triple: 700,
  drop: 400,
};

function playNote(
  ctx: AudioContext,
  freq: number,
  type: OscillatorType,
  startTime: number,
  duration: number,
  volume: number,
) {
  const osc = ctx.createOscillator();
  const gain = ctx.createGain();
  osc.type = type;
  osc.frequency.value = freq;
  gain.gain.setValueAtTime(volume, startTime);
  gain.gain.exponentialRampToValueAtTime(0.001, startTime + duration);
  osc.connect(gain).connect(ctx.destination);
  osc.start(startTime);
  osc.stop(startTime + duration);
}

function chimeDingDong(ctx: AudioContext) {
  const now = ctx.currentTime;
  playNote(ctx, 880, "sine", now, 0.3, 0.18);        // A5
  playNote(ctx, 659.25, "sine", now + 0.15, 0.35, 0.18); // E5
}

function chimeSoftBell(ctx: AudioContext) {
  const now = ctx.currentTime;
  playNote(ctx, 1174.66, "sine", now, 0.45, 0.12);   // D6
  playNote(ctx, 880, "triangle", now + 0.05, 0.4, 0.08); // A5 harmonic
}

function chimeTriple(ctx: AudioContext) {
  const now = ctx.currentTime;
  playNote(ctx, 783.99, "sine", now, 0.25, 0.15);       // G5
  playNote(ctx, 987.77, "sine", now + 0.18, 0.25, 0.15); // B5
  playNote(ctx, 1174.66, "sine", now + 0.36, 0.3, 0.15); // D6
}

function chimeDrop(ctx: AudioContext) {
  const now = ctx.currentTime;
  const osc = ctx.createOscillator();
  const gain = ctx.createGain();
  osc.type = "sine";
  osc.frequency.setValueAtTime(1400, now);
  osc.frequency.exponentialRampToValueAtTime(600, now + 0.25);
  gain.gain.setValueAtTime(0.2, now);
  gain.gain.exponentialRampToValueAtTime(0.001, now + 0.3);
  osc.connect(gain).connect(ctx.destination);
  osc.start(now);
  osc.stop(now + 0.3);
}

const CHIME_FNS: Record<ChimePreset, (ctx: AudioContext) => void> = {
  ding_dong: chimeDingDong,
  soft_bell: chimeSoftBell,
  triple: chimeTriple,
  drop: chimeDrop,
};

/** Play a chime preset. Returns a promise that resolves after the chime finishes. */
export function playChime(preset: ChimePreset): Promise<void> {
  return new Promise((resolve) => {
    try {
      const ctx = new AudioContext();
      CHIME_FNS[preset](ctx);
      setTimeout(() => {
        ctx.close();
        resolve();
      }, CHIME_DURATIONS[preset]);
    } catch {
      resolve();
    }
  });
}

// ── TTS (via Rust backend `say` command) ─────────────────────────────────────

export type TtsMode = "chime_and_speech" | "chime_only" | "off";

export interface TtsVoice {
  name: string;
  lang: string;
}

/** Get available TTS voices from the system via Tauri backend. */
export async function getVoices(): Promise<TtsVoice[]> {
  const { invoke } = await import("@tauri-apps/api/core");
  const locale = i18n.language === "zh" ? "zh" : "en";
  try {
    return await invoke<TtsVoice[]>("get_tts_voices", { locale });
  } catch {
    return [];
  }
}

/** Speak text using the system `say` command via Tauri backend. */
export async function speakText(text: string, voice?: string) {
  const { invoke } = await import("@tauri-apps/api/core");
  const locale = i18n.language === "zh" ? "zh" : "en";
  try {
    await invoke("speak_text", { text, voice: voice || null, locale });
  } catch {
    // silently ignore if backend unavailable
  }
}

/** Play alert: chime (optional) → speech (optional), based on current settings. */
export async function playAlertSound(summary: string) {
  if (getItem("overlay-muted") === "true") return;
  const mode = (getItem("tts-mode") as TtsMode) || "off";
  if (mode === "off") return;

  const chime = (getItem("chime-sound") as ChimePreset) || "ding_dong";
  await playChime(chime);

  if (mode === "chime_and_speech") {
    const voiceURI = getItem("tts-voice") || undefined;
    speakText(summary, voiceURI);
  }
}
