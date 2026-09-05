// 带语音输入的 textarea —— 给**没有工具行**的输入面用。
//
// 两个 composer（新会话 / 续写）自己有一行「附件 · 语音」工具栏，录音条直接接管
// 那一行。但决策卡里那些输入框是裸 textarea：拒绝理由、驳回意见、自由回答、
// 表单字段。它们同样是「说给 AI 听的自然语言」，而且**回答决策卡是手机上最高频
// 的输入场景**，所以这些也得能说 —— 而且要跟 composer 是同一套交互，不能这里
// 一种手势那里另一种。
//
// 待命时麦克风浮在右下角（DeepSeek / Kimi 的位置），录音时录音条压在输入框底部
// 那一条：都不必给每个调用点加一行工具栏，也不改动它们既有的布局与样式。
//
// 语音不可用时（浏览器不支持、无 GMS 的安卓机）退化成一个普通 textarea，连内边距
// 都不会多留。

import { useFollowTail, useVoiceRecorder } from "../useVoiceRecorder";
import { VoiceBar, VoiceMicButton } from "./VoiceBar";
import styles from "./VoiceTextarea.module.css";

export function VoiceTextarea({
  value,
  onChange,
  className,
  placeholder,
  rows,
  lang = "zh-CN",
  "aria-label": ariaLabel,
}: {
  value: string;
  onChange: (next: string) => void;
  /** 调用点原有的 textarea class 原样透传，样式不受本组件影响。 */
  className?: string;
  placeholder?: string;
  rows?: number;
  lang?: string;
  "aria-label"?: string;
}) {
  const rec = useVoiceRecorder({ lang, value, onChange });
  const taRef = useFollowTail<HTMLTextAreaElement>(rec.recording, rec.preview);

  return (
    <div className={styles.wrap}>
      <textarea
        ref={taRef}
        className={className}
        placeholder={placeholder}
        value={rec.recording ? rec.preview : value}
        readOnly={rec.recording}
        onChange={(e) => onChange(e.target.value)}
        rows={rows}
        aria-label={ariaLabel}
        // 给右下角的麦克风让位，否则最后一行文字会被压在按钮底下；录音时改成
        // 给底部那条录音条让位。
        style={
          rec.active
            ? { paddingBottom: 56 }
            : rec.available
              ? { paddingRight: 48 }
              : undefined
        }
      />
      {rec.active ? (
        <div className={styles.barSlot}>
          <VoiceBar rec={rec} />
        </div>
      ) : (
        rec.available && (
          <div className={styles.slot}>
            <VoiceMicButton rec={rec} />
          </div>
        )
      )}
    </div>
  );
}
