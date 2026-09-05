// 语音输入的两个界面：待命时的麦克风按钮，录音时接管整行的录音条。
//
// 为什么是「点一下」而不是「按住说话」：Fleet 的语音是**听写**——音频用完即弃，
// 产物是输入框里那段可以再改的文字。按住说话是*发语音条*的语法（那里音频本身
// 就是要发出去的东西）。两者的差别不是风格问题，按住会连带丢掉三样东西：录音时
// 没法滚动去看上文、手指一抬或被系统弹窗打断就中断（鸿蒙的麦克风权限框正是这么
// 把一次录音变成黑洞的）、以及说完之后从容修改的那口气。ChatGPT iOS 改成必须
// 按住之后，社区的抱怨逐条对上了这三点。
//
// 所以点按是主路径，长按保留成快捷方式：短句按住说完松手即收，一个动作最快。
// 老的「上滑取消」删掉了——盲手势，没有可视落点（连微信上滑都会浮出「取消 /
// 转文字」两个看得见的区域）。取消现在是录音条上一个明确的 ✕。

import { useRef } from "react";
import { AudioLines, RotateCcw, Send, Square, TriangleAlert, X } from "lucide-react";
import { t } from "../i18n";
import { voiceErrorHint, voiceErrorText } from "../useVoiceInput";
import { formatDuration, pressIntent, type VoiceRecorderApi } from "../useVoiceRecorder";
import styles from "./VoiceBar.module.css";

/** 波形的柱子数。固定根数、用 CSS 错开动画相位，不需要真音量数据。 */
const BARS = 14;

/**
 * 待命时的麦克风按钮。
 *
 * 点一下进录音态；按住不放则是「按住说话」，松手即收。两种意图靠松手时长区分
 * （pressIntent），所以用户不必先选一种用法。
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
          // 指针捕获:手指滑出按钮范围后仍然收得到 up,否则松手事件丢了,长按
          // 用法下录音会一直开着。**排在 start 之后**且吞掉异常:它是个锦上添花的
          // 增强,不能因为某个环境不认这个 pointerId 就把整个开始录音的动作带崩。
          try {
            e.currentTarget.setPointerCapture?.(e.pointerId);
          } catch {
            /* 捕获不到就算了,点按路径不依赖它 */
          }
        }}
        onPointerUp={() => {
          if (pressIntent(Date.now() - pressedAt.current) === "stop") rec.stop();
        }}
        // 指针被系统抢走（来电、手势返回）时按点按处理：录音继续，录音条已经在
        // 屏幕上，用户随时能停。**不要**在这里 cancel —— 鸿蒙的权限弹窗就会打断
        // 指针，一取消就把刚授权好的这次录音丢了。
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
 * 出错时那一块。
 *
 * 原来是一行小红字加个「重试」——对「没有麦克风权限」这一类来说，重试是**没有
 * 用的动作**：用户拒过一次之后，鸿蒙和安卓的权限框都不会再弹了，再点一百次
 * 也还是同一条错误。所以这里按错误类型给不同的下一步：
 *
 *   - 能把用户送去授权的环境（原生壳）：给「去授权」，授权完自动接着录。
 *   - 送不过去的（浏览器）：给一句**说清是哪个设置**的指引，不画按不动的按钮。
 *   - 这台设备根本没有识别服务：连重试都不给，说清楚改用打字。
 */
function VoiceError({ rec }: { rec: VoiceRecorderApi }) {
  const hint = voiceErrorHint(rec.error!, rec.canOpenSettings, rec.providerId);
  // 重试只在「再来一次真的有可能不一样」时才给。unavailable 是设备就没有识别
  // 服务,no-permission 且送不过去时用户得先离开这个界面去改设置 —— 这两种
  // 情况下的重试按钮只会骗人再点一次。
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
 * 录音条。接管输入框下面那一行。
 *
 * 波形是**活性指示**，不是音量表：Web Speech 不给音量，壳里的引擎自己占着麦克风，
 * 我们再开一路 getUserMedia 去测音量有抢麦的风险。它挂靠的是另一个真信号——引擎
 * 在不在出字（rec.speaking）。真正的「听见了」证据是实时转写，而那段字上在真正的
 * 输入框里，不在这条上。
 */
export function VoiceBar({ rec }: { rec: VoiceRecorderApi }) {
  // 引擎还没说「麦克风开了」。**不走秒也不画波形** —— 那两样都在说「我在听你
  // 说」，而这一刻它并没有；用户照着假信号开口，前半句就丢了。留一个 ✕，因为
  // 这段可能卡在权限框或端侧模型加载上，得让人退得出来。
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

  // 收音已经停了，还在等引擎把最后半句定稿吐出来。条子留在原地并说明在干什么，
  // 否则用户按下发送后要对着一个毫无动静的界面等一秒多。
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
