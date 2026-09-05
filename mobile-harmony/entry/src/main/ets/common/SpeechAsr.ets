/// 端侧语音识别 —— mobile-web/src/voiceHarmony.ts 的原生一端。
///
/// 走 Core Speech Kit 的 speechRecognizer：模型内置在系统里，离线可用、免费、
/// 无配额。这是鸿蒙端相对另外两条 provider 的优势（Web Speech 要把音频发去厂商
/// 服务器，Android 那条依赖 GMS）。
///
/// 代价是**只识别中文**：Core Speech Kit 官方明确仅支持中文，英文/日文要另接
/// 第三方云 SDK。而 Fleet 的语音内容天然中英混排（「合一下 worktree」），所以
/// 英文标识符的识别质量是这条路已知的短板，不是可以修的 bug。
///
/// `recognitionMode: 0` 让引擎**自己采集麦克风**，我们不必接 AudioCapturer 自己
/// 喂 PCM（那条路要按 640/1280 字节切块 writeAudio，代码量大得多）。

import { speechRecognizer } from '@kit.CoreSpeechKit';
import { abilityAccessCtrl, common, Permissions } from '@kit.AbilityKit';
import { hilog } from '@kit.PerformanceAnalysisKit';

const DOMAIN = 0x0000;
const TAG = 'fleet-asr';
const MIC: Permissions = 'ohos.permission.MICROPHONE';

/// 引擎参数。取值来自 SDK 的 @hms.ai.speechRecognizer.d.ts 与官方示例：
/// audioType 目前只支持 pcm，且必须 16kHz / 16bit / 单声道。
const SAMPLE_RATE = 16000;
const SAMPLE_BIT = 16;
const CHANNEL = 1;
/// online 字段：0 = 在线，1 = 离线。我们要离线那条。
const OFFLINE = 1;

export interface AsrCallbacks {
  onText: (text: string, isFinal: boolean) => void;
  onError: (code: string) => void;
  /// 引擎自己收工了（VAD 判定说完了、或到了 maxAudioDuration）。
  onComplete: () => void;
}

/// 一次识别的持有者。壳里只需要一个实例。
export class SpeechAsr {
  private engine: speechRecognizer.SpeechRecognitionEngine | null = null;
  private sessionId: string = '';
  /// 取消之后引擎仍会把回调送完。调用方已经说了不要，之后一律咽掉。
  private dead: boolean = false;

  /// 申请麦克风权限。已授权时直接过，用户拒过则返回 false。
  private async ensureMic(ctx: common.UIAbilityContext): Promise<boolean> {
    try {
      const mgr = abilityAccessCtrl.createAtManager();
      const res = await mgr.requestPermissionsFromUser(ctx, [MIC]);
      // 0 = 已授权。数组与请求的权限一一对应，这里只请求了一个。
      return res.authResults.length > 0 && res.authResults[0] === 0;
    } catch (e) {
      hilog.warn(DOMAIN, TAG, 'mic permission request failed: %{public}s', JSON.stringify(e));
      return false;
    }
  }

  /// 开始识别。`lang` 形如 `zh-CN`。
  async start(ctx: common.UIAbilityContext, lang: string, cb: AsrCallbacks): Promise<void> {
    // 诊断用的时间线（计划 harmony-voice-diag）。「弹了权限却一个字都出不来」有
    // 四条互相长得一样的可能路径，光看界面分不出来；这几行把它们分开：
    //   permission 花了多久（>1s 基本就是弹了窗，说明用户那一刻手指必然离开了按钮）
    //   createEngine 花了多久（用户开口早于引擎就绪，前半句会丢）
    //   startListening 有没有发出去、之后有没有回调
    const t0 = Date.now();
    hilog.info(DOMAIN, TAG, 'voice-diag start lang=%{public}s', lang);
    const granted = await this.ensureMic(ctx);
    hilog.info(DOMAIN, TAG, 'voice-diag permission granted=%{public}s cost=%{public}dms',
      granted ? 'yes' : 'no', Date.now() - t0);
    if (!granted) {
      cb.onError('PERMISSION_DENIED');
      return;
    }
    this.dead = false;
    // sessionId 每次换新:引擎按它区分回调归属,复用会让上一轮的尾巴混进来。
    this.sessionId = `fleet-${Date.now()}`;
    const session = this.sessionId;

    try {
      // 引擎可以复用，但语言是 createEngine 时定死的，而调用方可能换语言，
      // 所以每次重建。shutdown 在 stop/cancel 里做。
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
      hilog.info(DOMAIN, TAG, 'voice-diag createEngine ok cost=%{public}dms dead=%{public}s',
        Date.now() - tEngine, this.dead ? 'yes' : 'no');
      this.engine = engine;

      engine.setListener({
        onStart: (_sid: string, _msg: string) => {
          hilog.info(DOMAIN, TAG, 'voice-diag engine onStart cost=%{public}dms', Date.now() - t0);
        },
        onEvent: (_sid: string, code: number, _msg: string) => {
          hilog.info(DOMAIN, TAG, 'voice-diag engine event code=%{public}d', code);
        },
        onResult: (sid: string, result: speechRecognizer.SpeechRecognitionResult) => {
          // 丢弃也要留痕：dead=yes 正是「页面已放弃、引擎还在录」那条路径的证据。
          hilog.info(DOMAIN, TAG, 'voice-diag result final=%{public}s len=%{public}d dead=%{public}s match=%{public}s',
            result.isFinal ? 'yes' : 'no', result.result.length,
            this.dead ? 'yes' : 'no', sid === session ? 'yes' : 'no');
          if (this.dead || sid !== session) { return; }
          cb.onText(result.result, result.isFinal);
        },
        onComplete: (sid: string, _msg: string) => {
          hilog.info(DOMAIN, TAG, 'voice-diag complete dead=%{public}s cost=%{public}dms',
            this.dead ? 'yes' : 'no', Date.now() - t0);
          if (this.dead || sid !== session) { return; }
          cb.onComplete();
        },
        onError: (sid: string, code: number, msg: string) => {
          if (this.dead || sid !== session) { return; }
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
      // recognitionMode 0 = 引擎自采麦克风。vadEnd 是「静默多久算说完」——
      // 3 秒对着手机想下一句话是常事，短了会把一句话切两半。
      const extra: Record<string, Object> = {
        'recognitionMode': 0,
        'vadBegin': 2000,
        'vadEnd': 3000,
        'maxAudioDuration': 60000,
      };
      const startParams: speechRecognizer.StartParams = {
        sessionId: session,
        audioInfo: audioInfo,
        extraParams: extra,
      };
      engine.startListening(startParams);
      hilog.info(DOMAIN, TAG, 'voice-diag startListening issued cost=%{public}dms', Date.now() - t0);
    } catch (e) {
      hilog.warn(DOMAIN, TAG, 'asr start failed: %{public}s', JSON.stringify(e));
      cb.onError('START_FAILED');
    }
  }

  /// 正常收尾：让引擎把最后一段定稿吐出来（onResult isFinal）再收工。
  stop(): void {
    // engine=null 时这一下就是空转 —— 页面以为收了尾，实际引擎要么还没建起来
    // （随后照样会开始录音），要么早已收工。两种都得看得见。
    hilog.info(DOMAIN, TAG, 'voice-diag stop engine=%{public}s', this.engine === null ? 'null' : 'live');
    if (this.engine === null) { return; }
    try {
      this.engine.finish(this.sessionId);
    } catch (e) {
      hilog.warn(DOMAIN, TAG, 'asr finish failed: %{public}s', JSON.stringify(e));
    }
  }

  /// 丢弃这次识别。之后引擎补的回调都不再上报。
  cancel(): void {
    // 同 stop：engine=null 的 cancel 只置了 dead，而 start() 在拿到权限之后会把
    // dead 重新置回 false —— 这一行加上 start 那边的 permission cost，就足以判定
    // 「权限弹窗期间页面取消了、原生却照常开录」这条路径。
    hilog.info(DOMAIN, TAG, 'voice-diag cancel engine=%{public}s', this.engine === null ? 'null' : 'live');
    this.dead = true;
    if (this.engine === null) { return; }
    try {
      this.engine.cancel(this.sessionId);
      this.engine.shutdown();
    } catch (e) {
      hilog.warn(DOMAIN, TAG, 'asr cancel failed: %{public}s', JSON.stringify(e));
    }
    this.engine = null;
  }

  /// 引擎自己收工之后释放资源。留着不放会一直占着麦克风。
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
