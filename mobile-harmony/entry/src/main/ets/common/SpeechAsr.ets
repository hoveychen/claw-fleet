/// On-device speech recognition — the native side of mobile-web/src/voiceHarmony.ts.
///
/// Uses the Core Speech Kit's speechRecognizer: the model is built into the system,
/// works offline, is free, and has no quota. This is the advantage of HarmonyOS
/// relative to the other two providers (Web Speech sends audio to vendor servers,
/// Android depends on GMS).
///
/// The trade-off is **Chinese only**: Core Speech Kit officially supports Chinese
/// only; English/Japanese require a third-party cloud SDK. Since Fleet's voice
/// content is naturally mixed Chinese/English (「merge a worktree」), the recognition
/// quality of English identifiers is a known limitation, not a bug we can fix.
///
/// `recognitionMode: 0` lets the engine **capture the microphone itself**, so we
/// don't need to hook AudioCapturer and feed PCM manually (that path requires
/// splitting by 640/1280 bytes per writeAudio call, much more code).

import { speechRecognizer } from '@kit.CoreSpeechKit';
import { abilityAccessCtrl, common, Permissions } from '@kit.AbilityKit';
import { hilog } from '@kit.PerformanceAnalysisKit';
import { BusinessError } from '@kit.BasicServicesKit';

const DOMAIN = 0x0000;
const TAG = 'fleet-asr';
const MIC: Permissions = 'ohos.permission.MICROPHONE';

/// Engine parameters. Values from SDK @hms.ai.speechRecognizer.d.ts and official examples:
/// audioType currently supports only pcm, and must be 16kHz / 16bit / mono.
const SAMPLE_RATE = 16000;
const SAMPLE_BIT = 16;
const CHANNEL = 1;
/// online field: 0 = online, 1 = offline. We want offline.
const OFFLINE = 1;

export interface AsrCallbacks {
  /// Engine is actually recording. createEngine is async, so speech before that
  /// is lost; the page must wait for this callback before saying "listening".
  onReady: () => void;
  onText: (text: string, isFinal: boolean) => void;
  onError: (code: string) => void;
  /// Engine has finished (VAD determined speech is done, or maxAudioDuration reached).
  onComplete: () => void;
}

/// Holder for a single recognition session. The shell needs only one instance.
export class SpeechAsr {
  private engine: speechRecognizer.SpeechRecognitionEngine | null = null;
  private sessionId: string = '';
  /// A new generation number on each start / cancel call.
  ///
  /// Can't use a single `dead` boolean: start() has two awaits (request permission,
  /// create engine), and **the permission request will almost certainly be cancelled
  /// mid-wait** — the system permission dialog takes focus, the page receives
  /// pointercancel on the press, and the web side calls cancel(). With a boolean,
  /// cancel() sets `dead=true`, but that still-running start() resets it to
  /// `dead=false`, so after the user taps "allow" the engine still starts recording
  /// even though the page already tore down `window.__fleetVoice`, yielding only
  /// NO_HOOK — the symptom is "after the permission dialog, nothing comes out no
  /// matter how you tap" while the microphone stays locked until maxAudioDuration.
  /// Generation numbers are monotonic, so a late round can't change an earlier one.
  ///
  /// The `stale=yes` in voice-diag logs below marks "this round is superseded",
  /// the trail left by the earlier path.
  private gen: number = 0;
  /// Engine not yet built when stop is requested. See stop().
  private pendingStop: boolean = false;

  /// Request microphone permission. Returns true if already granted; false if user rejected.
  private async ensureMic(ctx: common.UIAbilityContext): Promise<boolean> {
    try {
      const mgr = abilityAccessCtrl.createAtManager();
      const res = await mgr.requestPermissionsFromUser(ctx, [MIC]);
      // 0 = already authorized. Array maps to requested permissions one-to-one; we request one.
      return res.authResults.length > 0 && res.authResults[0] === 0;
    } catch (e) {
      hilog.warn(DOMAIN, TAG, 'mic permission request failed: %{public}s', JSON.stringify(e));
      return false;
    }
  }

  /// Second-time authorization: user already rejected once, so bring up the
  /// system settings panel again.
  ///
  /// Why a separate entry: after user rejection, `requestPermissionsFromUser`
  /// **stops prompting** and just returns denial. The page's "please allow in
  /// system settings" becomes a dead end — user doesn't know where to go, and
  /// we can't send them there. `requestPermissionOnSetting` is the official
  /// path for this, and it opens the system panel in-app without leaving settings.
  ///
  /// Two branches that are "exceptions that are normal results"
  /// (error codes in @ohos.abilityAccessCtrl.d.ts):
  ///   - 12100011 all permissions already granted — already ok, return true.
  ///   - 12100012 user never rejected — API refuses this, fall back to the
  ///     normal request to prompt the first dialog.
  async requestOnSetting(ctx: common.UIAbilityContext): Promise<boolean> {
    const mgr = abilityAccessCtrl.createAtManager();
    try {
      const res = await mgr.requestPermissionOnSetting(ctx, [MIC]);
      return res.length > 0 && res[0] === abilityAccessCtrl.GrantStatus.PERMISSION_GRANTED;
    } catch (e) {
      const code = (e as BusinessError).code;
      if (code === 12100011) { return true; }
      if (code === 12100012) { return await this.ensureMic(ctx); }
      hilog.warn(DOMAIN, TAG, 'requestPermissionOnSetting failed: %{public}d', code);
      return false;
    }
  }

  /// Start recognition. `lang` is like `zh-CN`.
  async start(ctx: common.UIAbilityContext, lang: string, cb: AsrCallbacks): Promise<void> {
    // Timeline for diagnostics (planned as harmony-voice-diag). "Permission dialog
    // appeared but no text comes out" has four similar possible paths that look the
    // same on the UI; these lines tell them apart:
    //   permission cost in ms (>1s basically means dialog appeared, proving the
    //     user's finger left the button at that moment)
    //   createEngine cost (user spoke before engine was ready, first part is lost)
    //   startListening was sent and got a callback
    // Claim a gen at the start: every subsequent await compares gen to the current
    // one; mismatch means the caller doesn't want this recognition anymore.
    const gen = ++this.gen;
    this.pendingStop = false;
    const t0 = Date.now();
    hilog.info(DOMAIN, TAG, 'voice-diag start lang=%{public}s', lang);
    const granted = await this.ensureMic(ctx);
    hilog.info(DOMAIN, TAG, 'voice-diag permission granted=%{public}s cost=%{public}dms',
      granted ? 'yes' : 'no', Date.now() - t0);
    if (!granted) {
      if (gen !== this.gen) { return; }
      cb.onError('PERMISSION_DENIED');
      return;
    }
    // Cancelled while waiting for permission — **must not** continue down.
    // The next step is actual recording, but the page's callback hook is already torn down,
    // so there's no outlet for what gets recorded.
    if (gen !== this.gen) {
      hilog.info(DOMAIN, TAG, 'voice-diag abandoned after permission cost=%{public}dms', Date.now() - t0);
      return;
    }
    // New sessionId each time: the engine distinguishes callback ownership by it;
    // reuse would let the tail of the previous round bleed in.
    this.sessionId = `fleet-${Date.now()}`;
    const session = this.sessionId;

    try {
      // Engine can be reused, but language is fixed at createEngine time; if the caller
      // changes language, we rebuild. Shutdown happens in stop/cancel.
      if (this.engine !== null) {
        this.engine.shutdown();
        this.engine = null;
      }
      const createParams: speechRecognizer.CreateEngineParams = {
        language: lang,
        online: OFFLINE,
      };
      const tEngine = Date.now();
      const engine = await speechRecognizer.createEngine(createParams);
      hilog.info(DOMAIN, TAG, 'voice-diag createEngine ok cost=%{public}dms stale=%{public}s',
        Date.now() - tEngine, gen !== this.gen ? 'yes' : 'no');
      // On-device model loading takes time too; can be cancelled here as well. Engine
      // is already built, so we must shut it down, otherwise it keeps the mic locked.
      if (gen !== this.gen) {
        engine.shutdown();
        return;
      }
      this.engine = engine;

      engine.setListener({
        // onStart is when the engine announces "mic is really on"; the page advances
        // from "preparing" to "listening". createEngine loads the on-device model,
        // and there's a real gap; speech during that window won't enter results.
        onStart: (sid: string, _msg: string) => {
          hilog.info(DOMAIN, TAG, 'voice-diag engine onStart cost=%{public}dms', Date.now() - t0);
          if (gen !== this.gen || sid !== session) { return; }
          cb.onReady();
        },
        onEvent: (_sid: string, code: number, _msg: string) => {
          hilog.info(DOMAIN, TAG, 'voice-diag engine event code=%{public}d', code);
        },
        onResult: (sid: string, result: speechRecognizer.SpeechRecognitionResult) => {
          // Discard with breadcrumbs: stale=yes proves "page gave up, engine is still recording".
          hilog.info(DOMAIN, TAG, 'voice-diag result final=%{public}s len=%{public}d stale=%{public}s match=%{public}s',
            result.isFinal ? 'yes' : 'no', result.result.length,
            gen !== this.gen ? 'yes' : 'no', sid === session ? 'yes' : 'no');
          if (gen !== this.gen || sid !== session) { return; }
          cb.onText(result.result, result.isFinal);
        },
        onComplete: (sid: string, _msg: string) => {
          hilog.info(DOMAIN, TAG, 'voice-diag complete stale=%{public}s cost=%{public}dms',
            gen !== this.gen ? 'yes' : 'no', Date.now() - t0);
          if (gen !== this.gen || sid !== session) { return; }
          cb.onComplete();
        },
        onError: (sid: string, code: number, msg: string) => {
          if (gen !== this.gen || sid !== session) { return; }
          hilog.warn(DOMAIN, TAG, 'asr error %{public}d %{public}s', code, msg);
          cb.onError(`${code}`);
        },
      });

      const audioInfo: speechRecognizer.AudioInfo = {
        audioType: 'pcm',
        sampleRate: SAMPLE_RATE,
        soundChannel: CHANNEL,
        sampleBit: SAMPLE_BIT,
      };
      // recognitionMode 0 = engine captures mic itself. vadEnd is "how much silence
      // means speech is done".
      //
      // 5 seconds not 3: official guidance per scenario is 2s for commands, **5s for
      // dictation**, and Fleet's voice is dictation — pausing 3s while thinking of
      // the next sentence is normal. Too short and the engine finishes on its own,
      // losing what the user says next. Better to wait an extra 2 seconds (user can
      // tap stop anytime) than cut a sentence in half.
      const extra: Record<string, Object> = {
        'recognitionMode': 0,
        'vadBegin': 2000,
        'vadEnd': 5000,
        'maxAudioDuration': 60000,
      };
      const startParams: speechRecognizer.StartParams = {
        sessionId: session,
        audioInfo: audioInfo,
        extraParams: extra,
      };
      engine.startListening(startParams);
      hilog.info(DOMAIN, TAG, 'voice-diag startListening issued cost=%{public}dms', Date.now() - t0);
      // User tapped stop before engine was ready — no engine to stop yet, just left
      // a flag here. Without this, that stop is a no-op: page resets while engine
      // just started recording, locks until maxAudioDuration.
      if (this.pendingStop) {
        this.pendingStop = false;
        hilog.info(DOMAIN, TAG, 'voice-diag deferred stop applied');
        this.stop();
      }
    } catch (e) {
      hilog.warn(DOMAIN, TAG, 'asr start failed: %{public}s', JSON.stringify(e));
      if (gen !== this.gen) { return; }
      cb.onError('START_FAILED');
    }
  }

  /// Normal wrap-up: let engine emit the final segment (onResult isFinal) then finish.
  stop(): void {
    // engine=null is a no-op — page thinks it's wrapped up, but the engine either
    // hasn't been built yet (will start recording anyway) or already finished.
    // Both need to be visible.
    hilog.info(DOMAIN, TAG, 'voice-diag stop engine=%{public}s', this.engine === null ? 'null' : 'live');
    if (this.engine === null) {
      // Engine still building (createEngine loads the on-device model, not instant).
      // Mark it; start() will stop for us at the bottom — otherwise this is a no-op:
      // page resets while engine soon starts recording anyway.
      this.pendingStop = true;
      return;
    }
    try {
      this.engine.finish(this.sessionId);
    } catch (e) {
      hilog.warn(DOMAIN, TAG, 'asr finish failed: %{public}s', JSON.stringify(e));
    }
  }

  /// Discard this recognition. Subsequent engine callbacks are not reported.
  cancel(): void {
    // Bump gen and the in-flight start() abandons at the next checkpoint
    // (before we only had a dead boolean, and start() reset it to false after
    // getting permission — that's the "page cancelled during permission dialog,
    // but recording still starts" path). This log paired with permission cost
    // from start() still lets us identify the path on real hardware.
    hilog.info(DOMAIN, TAG, 'voice-diag cancel engine=%{public}s', this.engine === null ? 'null' : 'live');
    this.gen++;
    this.pendingStop = false;
    if (this.engine === null) { return; }
    try {
      this.engine.cancel(this.sessionId);
      this.engine.shutdown();
    } catch (e) {
      hilog.warn(DOMAIN, TAG, 'asr cancel failed: %{public}s', JSON.stringify(e));
    }
    this.engine = null;
  }

  /// Release resources after engine finishes. Leaving it live keeps the mic locked.
  release(): void {
    if (this.engine === null) { return; }
    try {
      this.engine.shutdown();
    } catch (e) {
      hilog.warn(DOMAIN, TAG, 'asr shutdown failed: %{public}s', JSON.stringify(e));
    }
    this.engine = null;
  }
}
