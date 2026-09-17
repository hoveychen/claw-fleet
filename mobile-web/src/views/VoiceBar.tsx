// Two interfaces for voice input: microphone button when idle, recording bar
// that takes over the whole input line when recording.
//
// Why "tap" instead of "hold to speak": Fleet's voice is **dictation** — audio
// is discarded after use, the product is editable text in the input box. Hold to
// speak is the *send voice message* syntax (where the audio itself is what gets
// sent). The difference is not stylistic; hold brings three losses: can't scroll
// to see context while recording, hand lift or system dialog interrupt ends it
// (HarmonyOS mic permission prompt does exactly this to a recording), and the
// ability to edit after finishing. After ChatGPT iOS switched to mandatory hold,
// community complaints lined up precisely on these three points.
//
// So tap is the main path, hold is preserved as a shortcut: tap to type (hold
// for quick short sentences). The old "swipe up to cancel" was removed — blind
// gesture with no visual target (even WeChat shows visible "cancel/transcribe"
// regions). Cancel is now a clear ✕ on the recording bar.

import { useRef } from "react";
import { AudioLines, RotateCcw, Send, Square, TriangleAlert, X } from "lucide-react";
import { t } from "../i18n";
import { voiceErrorHint, voiceErrorText } from "../useVoiceInput";
import { formatDuration, pressIntent, type VoiceRecorderApi } from "../useVoiceRecorder";
import styles from "./VoiceBar.module.css";

/** Waveform bar count. Fixed number, CSS staggers animation phases — no need for
 *  actual volume data. */
const BARS = 14;

/**
 * Microphone button in idle state.
 *
 * Tap to enter recording mode; hold is "hold to speak", release to stop. Both
 * intents are distinguished by release duration (pressIntent), so the user
 * doesn't have to choose upfront.
 */
export function VoiceMicButton({ rec }: { rec: VoiceRecorderApi }) {
  const pressedAt = useRef(0);

  return (
    <>
      <button
        type="button"
        className={styles.mic}
        onPointerDown={(e) => {
          pressedAt.current = Date.now();
          rec.start();
          // Pointer capture: get up events even if finger slides out of button,
          // otherwise release is lost and recording stays on in hold mode.
          // **Placed after start** and swallows exceptions: it's a nice-to-have
          // enhancement; don't break the whole start-recording action if an
          // environment doesn't recognize this pointerId.
          try {
            e.currentTarget.setPointerCapture?.(e.pointerId);
          } catch {
            /* If capture fails, so be it; tap path doesn't depend on it */
          }
        }}
        onPointerUp={() => {
          if (pressIntent(Date.now() - pressedAt.current) === "stop") rec.stop();
        }}
        // When system takes pointer (incoming call, gesture back), treat as tap:
        // recording continues, bar is on screen, user can stop anytime.
        // **Don't** cancel here — HarmonyOS permission dialog interrupts pointer,
        // cancel discards the recording that just got permission.
        onPointerCancel={() => {}}
        onContextMenu={(e) => e.preventDefault()}
        aria-label={t("语音输入")}
      >
        <AudioLines size={19} />
      </button>
      {rec.error && <VoiceError rec={rec} />}
    </>
  );
}

/**
 * Error display block.
 *
 * It used to be a small red line plus a "retry" button — for "no microphone
 * permission" type errors, retry is **not a useful action**: after user denies
 * once, HarmonyOS and Android permission dialogs don't show again, clicking
 * 100 more times gives the same error. So we offer different next steps by
 * error type:
 *
 *   - Environments that can send users to authorize (native shell): offer
 *     "authorize", auto-resume recording after grant.
 *   - Ones that can't (browser): give **clear instructions which setting**,
 *     no disabled button.
 *   - Device has no recognition service at all: no retry, suggest typing instead.
 */
function VoiceError({ rec }: { rec: VoiceRecorderApi }) {
  const hint = voiceErrorHint(rec.error!, rec.canOpenSettings, rec.providerId);
  // Only show retry when "trying again might actually be different".
  // unavailable = device has no recognition service; no-permission without
  // canOpenSettings = user must leave this view to change settings — in both
  // cases, the retry button just tricks them into clicking again.
  const canRetry = rec.error !== "unavailable" && !(rec.error === "no-permission" && !rec.canOpenSettings);

  return (
    <div className={styles.errorBox} role="alert">
      <div className={styles.errorHead}>
        <TriangleAlert size={15} className={styles.errorIcon} />
        <span className={styles.errorTitle}>{voiceErrorText(rec.error!)}</span>
        <button
          type="button"
          className={styles.errorClose}
          onClick={rec.dismissError}
          aria-label={t("关闭")}
        >
          <X size={14} />
        </button>
      </div>
      {hint && <p className={styles.errorHint}>{hint}</p>}
      <div className={styles.errorActs}>
        {rec.error === "no-permission" && rec.canOpenSettings && (
          <button
            type="button"
            className={styles.errorPrimary}
            onClick={() => void rec.openSettings()}
          >
            {t("去授权")}
          </button>
        )}
        {canRetry && (
          <button type="button" className={styles.errorGhost} onClick={rec.start}>
            {t("重试")}
          </button>
        )}
      </div>
    </div>
  );
}

/**
 * Recording bar. Takes over the line below the input box.
 *
 * Waveform is an **activity indicator**, not a volume meter: Web Speech doesn't
 * provide volume; the in-shell engine holds the mic exclusively; opening another
 * getUserMedia path for volume risks mic contention. It hooks into a different
 * true signal — whether the engine is outputting text (rec.speaking). The actual
 * "we heard you" proof is real-time transcription, which appears in the real
 * input box, not here.
 */
export function VoiceBar({ rec }: { rec: VoiceRecorderApi }) {
  // Engine hasn't confirmed "mic is open". **Don't show timer or waveform** —
  // both say "I'm listening" but it's not yet; user speaks to the false signal
  // and loses the first half. Keep a ✕ because this might hang on permission
  // dialog or model loading; user needs an exit.
  if (rec.preparing) {
    return (
      <div className={styles.bar} role="status">
        <button
          type="button"
          className={styles.act}
          onClick={rec.cancel}
          aria-label={t("取消录音")}
        >
          <X size={18} />
        </button>
        <span className={styles.finalizing}>{t("准备中…")}</span>
      </div>
    );
  }

  // Recording stopped, waiting for engine to finalize and output the last
  // sentence. Bar stays and shows what's happening; otherwise user sees a still
  // screen for over a second after pressing send.
  if (!rec.recording) {
    return (
      <div className={styles.bar} role="status">
        <span className={styles.finalizing}>{t("整理最后一句…")}</span>
      </div>
    );
  }

  return (
    <div className={styles.bar} role="group" aria-label={t("录音中")}>
      <button
        type="button"
        className={styles.act}
        onClick={rec.cancel}
        aria-label={t("取消录音")}
      >
        <X size={18} />
      </button>

      <div className={styles.wave} data-speaking={rec.speaking || undefined} aria-hidden="true">
        {Array.from({ length: BARS }, (_, i) => (
          <span key={i} style={{ animationDelay: `${(i % 7) * 0.09}s` }} />
        ))}
      </div>

      <span className={styles.time}>{formatDuration(rec.seconds)}</span>

      {rec.dirty && (
        <button
          type="button"
          className={styles.act}
          onClick={rec.retry}
          aria-label={t("重录")}
          title={t("重录")}
        >
          <RotateCcw size={17} />
        </button>
      )}

      <button
        type="button"
        className={styles.stop}
        onClick={rec.stop}
        aria-label={t("停止录音")}
      >
        <Square size={16} />
      </button>

      {rec.canSend && (
        <button
          type="button"
          className={styles.send}
          onClick={rec.stopAndSend}
          disabled={rec.finalizing}
          aria-label={t("停止并发送")}
        >
          <Send size={16} />
        </button>
      )}
    </div>
  );
}
