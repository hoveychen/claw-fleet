// 带语音输入的 textarea —— 给**没有工具行**的输入面用。
//
// 两个 composer（新会话 / 续写）自己有一行「附件 · 语音」工具栏，语音按钮直接
// 挂在那里。但决策卡里那些输入框是裸 textarea：拒绝理由、驳回意见、自由回答、
// 表单字段。它们同样是「说给 AI 听的自然语言」，而且**回答决策卡是手机上最高频
// 的输入场景**，所以这些也得能说。
//
// 按钮浮在右下角（DeepSeek / Kimi 的位置），因此不必给每个调用点加一行工具栏、
// 也不改动它们既有的布局与样式：换掉 <textarea> 标签本身就够。
//
// 语音不可用时（浏览器不支持、无 GMS 的安卓机）VoiceButton 自己返回 null，
// 这里退化成一个普通 textarea，连内边距都不会多留。

import { useRef } from "react";
import { appendVoiceText } from "../voiceInput";
import { useVoiceAvailable } from "../useVoiceInput";
import { VoiceButton } from "./VoiceButton";
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
  const ready = useVoiceAvailable() === "ready";
  // 识别结果是异步到的，期间用户可能又敲了字。闭包里的 value 会是那一刻的旧值，
  // 拿它拼接就把这几个字覆盖掉了 —— 所以读 ref 里最近一次渲染的值。
  const valueRef = useRef(value);
  valueRef.current = value;

  return (
    <div className={styles.wrap}>
      <textarea
        className={className}
        placeholder={placeholder}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        rows={rows}
        aria-label={ariaLabel}
        // 按钮浮在右下角，给它让出位置，否则最后一行文字会被压在按钮底下。
        style={ready ? { paddingRight: 42 } : undefined}
      />
      <div className={styles.slot}>
        <VoiceButton lang={lang} onText={(text) => onChange(appendVoiceText(valueRef.current, text))} />
      </div>
    </div>
  );
}
