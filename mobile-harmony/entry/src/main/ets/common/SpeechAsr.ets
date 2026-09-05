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
  /// 每次 start / cancel 换一代号。
  ///
  /// 不能用一个 `dead` 布尔:start() 里有两段 await(申请权限、创建引擎),而
  /// **申请权限那段几乎必然会被取消打断** —— 系统权限框一弹就抢走焦点,页面上
  /// 那次按压随之收到 pointercancel,web 侧于是 cancel()。布尔版本里 cancel 设的
  /// `dead=true` 会被那个还在 await 的 start() 一行 `dead=false` 重置掉,于是用户
  /// 点完「允许」引擎照常开录,而页面早已把 `window.__fleetVoice` 拆了,结果推回去
  /// 只剩 NO_HOOK —— 表现就是「弹过一次权限之后,怎么按都不出字」,而麦克风还占着
  /// 到 maxAudioDuration。代号是单调递增的,晚到的那一轮改不动它。
  private gen: number = 0;
  /// 引擎还没建好就被要求停止。见 stop()。
  private pendingStop: boolean = false;

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
    // 开头就占号:这一轮之后的每一次 await 都要拿它跟当前代号比,不一致说明调用方
    // 已经不要这次识别了。
    const gen = ++this.gen;
    this.pendingStop = false;
    if (!await this.ensureMic(ctx)) {
      if (gen !== this.gen) { return; }
      cb.onError('PERMISSION_DENIED');
      return;
    }
    // 等权限的这段时间里被取消了 —— **绝不能**继续往下走。往下一步就是真的开录,
    // 而页面那边的回调 hook 早已拆掉,录到的东西没有任何出口。
    if (gen !== this.gen) { return; }
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
      const engine = await speechRecognizer.createEngine(createParams);
      // 端侧模型的加载也要时间,这段里同样可能被取消。引擎已经建出来了,得还回去,
      // 否则它一直占着麦克风。
      if (gen !== this.gen) {
        engine.shutdown();
        return;
      }
      this.engine = engine;

      engine.setListener({
        onStart: (_sid: string, _msg: string) => {},
        onEvent: (_sid: string, _code: number, _msg: string) => {},
        onResult: (sid: string, result: speechRecognizer.SpeechRecognitionResult) => {
          if (gen !== this.gen || sid !== session) { return; }
          cb.onText(result.result, result.isFinal);
        },
        onComplete: (sid: string, _msg: string) => {
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
      // 引擎还没就绪时用户就按了停止 —— 那一下没有引擎可停,只留了个标记,补在这里。
      // 少了这一步,那次停止就是空操作:页面已经复位,引擎却刚开始录,一直录到
      // maxAudioDuration 才收工。
      if (this.pendingStop) {
        this.pendingStop = false;
        this.stop();
      }
    } catch (e) {
      hilog.warn(DOMAIN, TAG, 'asr start failed: %{public}s', JSON.stringify(e));
      if (gen !== this.gen) { return; }
      cb.onError('START_FAILED');
    }
  }

  /// 正常收尾：让引擎把最后一段定稿吐出来（onResult isFinal）再收工。
  stop(): void {
    if (this.engine === null) {
      // 引擎还在建(createEngine 要加载端侧模型,不是瞬时的)。记下来,start 走到
      // 底会替我们停。
      this.pendingStop = true;
      return;
    }
    try {
      this.engine.finish(this.sessionId);
    } catch (e) {
      hilog.warn(DOMAIN, TAG, 'asr finish failed: %{public}s', JSON.stringify(e));
    }
  }

  /// 丢弃这次识别。之后引擎补的回调都不再上报，进行中的 start 也会中途放弃。
  cancel(): void {
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
