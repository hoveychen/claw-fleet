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

/** Shared AudioContext — reused across chime calls to avoid autoplay-policy issues. */
let sharedCtx: AudioContext | null = null;

/** Get or create a running AudioContext.  If the existing context is stuck in
 *  "suspended" state (e.g. it was created outside a user-gesture on WKWebView),
 *  close it and create a fresh one so a subsequent resume() from a click handler
 *  can succeed. */
async function ensureAudioContext(): Promise<AudioContext> {
  if (!sharedCtx || sharedCtx.state === "closed") {
    sharedCtx = new AudioContext();
  }

  if (sharedCtx.state === "suspended") {
    // Race resume() against a short timeout — on WKWebView, resume() may
    // hang indefinitely when called outside a user gesture.
    const resumed = await Promise.race([
      sharedCtx.resume().then(() => true),
      new Promise<false>((r) => setTimeout(() => r(false), 300)),
    ]);
    if (!resumed || sharedCtx.state === "suspended") {
      // Context is stuck — tear it down and create a new one.
      console.debug("[audio] AudioContext stuck in suspended state, recreating");
      sharedCtx.close().catch(() => {});
      sharedCtx = new AudioContext();
      // This new context is created within the current (user-gesture) call
      // stack, so resume should succeed immediately.
      await sharedCtx.resume().catch(() => {});
    }
  }

  return sharedCtx;
}

/** Play a chime preset. Returns a promise that resolves after the chime finishes. */
export async function playChime(preset: ChimePreset): Promise<void> {
  try {
    const ctx = await ensureAudioContext();
    CHIME_FNS[preset](ctx);
    await new Promise((r) => setTimeout(r, CHIME_DURATIONS[preset]));
  } catch (err) {
    console.warn("[audio] playChime failed:", err);
  }
}

// ── TTS (via Microsoft Edge TTS) ─────────────────────────────────────────────

export type TtsMode = "chime_and_speech" | "chime_only" | "off";

export interface TtsVoice {
  name: string;
  lang: string;
  display_name: string;
  gender: string;
}

/** Get available TTS voices from Microsoft Edge TTS via Tauri backend. */
export async function getVoices(): Promise<TtsVoice[]> {
  const { invoke } = await import("@tauri-apps/api/core");
  const locale = i18n.language === "zh" ? "zh" : "en";
  try {
    return await invoke<TtsVoice[]>("get_tts_voices", { locale });
  } catch {
    return [];
  }
}

/** Synthesize and play text using Microsoft Edge TTS via Tauri backend.
 *  Audio playback happens on the Rust side (rodio) with automatic fallback to macOS `say`.
 */
export async function speakText(text: string, voice?: string) {
  const { invoke } = await import("@tauri-apps/api/core");
  const locale = i18n.language === "zh" ? "zh" : "en";
  try {
    await invoke("speak_text", { text, voice: voice || null, locale });
  } catch (err) {
    console.warn("[audio] speakText failed:", err);
  }
}

// ── Alert queue ─────────────────────────────────────────────────────────────

let alertQueue: string[] = [];
let alertPlaying = false;

async function processAlertQueue() {
  if (alertPlaying) return;
  alertPlaying = true;
  while (alertQueue.length > 0) {
    const summary = alertQueue.shift()!;
    await playAlertSoundImpl(summary);
  }
  alertPlaying = false;
}

/** Play alert: chime (optional) → speech (optional), based on current settings. */
export function playAlertSound(summary: string) {
  if (getItem("overlay-muted") === "true") {
    console.debug("[audio] alert skipped: overlay muted");
    return;
  }
  const mode = (getItem("tts-mode") as TtsMode) || "off";
  if (mode === "off") {
    console.debug("[audio] alert skipped: tts mode off");
    return;
  }
  alertQueue.push(summary);
  processAlertQueue();
}

async function playAlertSoundImpl(_summary: string) {
  const chime = (getItem("chime-sound") as ChimePreset) || "ding_dong";
  console.debug("[audio] playing chime:", chime);
  await playChime(chime);
  // TTS is now handled by the Rust backend after sending the notification.
}
