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
import { AudioLines, RotateCcw, Send, Square, X } from "lucide-react";
import { t } from "../i18n";
import { voiceErrorText } from "../useVoiceInput";
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
      {rec.error && (
        <span className={styles.error} role="alert">
          {voiceErrorText(rec.error)}
          <button type="button" className={styles.retryLink} onClick={rec.start}>
            {t("重试")}
          </button>
        </span>
      )}
    </>
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
