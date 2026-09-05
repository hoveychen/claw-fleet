// 输入框旁边的语音按钮。挂在 AttachmentRow 里，所以新会话 sheet 和续写
// composer 两处自动都有。
//
// 交互是**点一下开始、再点一下结束**，没有长按、没有滑动手势。
//
// 之前这里是「按住说话 + 快速点一下锁定 + 上滑取消」的合流，靠两个看不见的阈值
// （松手 ≤500ms 算点按、上滑 ≥60px 算取消）分流。三个问题让它站不住：
//
//   1. 两个阈值在界面上没有任何锚点。滑到哪儿算取消，只有滑过头之后才知道；
//      手快一点松开就静默进了锁定态，麦克风还开着而用户以为已经结束了。
//   2. 「按住说话 + 上滑取消」是**发语音消息**的手势语法（微信）。我们做的是
//      语音转文字，业界（iOS 听写、Gemini）在这件事上一律是点按开关。
//   3. 长按和系统权限弹窗天生冲突：弹窗一盖上来就抢走指针，浏览器派发
//      pointercancel，这一次录音在用户点「允许」之前就被判成取消了。
//      —— 鸿蒙端「弹了权限却一个字都出不来」的首要嫌疑正是这条。
//
// 环境探测、provider 选择、识别生命周期都在 useVoiceInput 里；这里只管画。

import { AudioLines, Loader, Square } from "lucide-react";
import { t } from "../i18n";
import { useVoiceInput, voiceErrorText } from "../useVoiceInput";
import type { VoiceState } from "../useVoiceInput";
import styles from "./VoiceButton.module.css";

/**
 * 按钮旁边那行状态文字。
 *
 * 抽成纯函数是为了能单测 —— 本仓的组件测试只有 renderToStaticMarkup，测不了
 * hook 的状态迁移，所以把「哪个状态说哪句话」这条真正会出错的规则单独拎出来。
 *
 * 「准备中」必须说出来：麦克风还没开，这段时间说的话会丢，用户有权知道现在
 * 别急着开口。之前这一段被画成「正在听」，代价是用户以为前半句被吞了。
 */
export function voiceHint(state: VoiceState, partial: string): string {
  if (state === "preparing") return t("准备中…");
  if (state === "listening") return partial || t("点击停止");
  return "";
}

export function VoiceButton({
  lang = "zh-CN",
  onText,
}: {
  lang?: string;
  /** 识别出的一段文字。调用方决定怎么并进输入框。 */
  onText: (text: string) => void;
}) {
  const { state, partial, error, start, stop } = useVoiceInput(lang, onText);

  // 探测没结束、或这台设备根本没有识别服务时,整个按钮不出现 —— 画一个按下去
  // 只会报错的按钮没有意义。
  if (state === "probing" || state === "unsupported") return null;

  const preparing = state === "preparing";
  const listening = state === "listening";
  // 「准备中」也算开着:这一下点下去是收，不是再开一次。
  const active = preparing || listening;

  const hint = voiceHint(state, partial);

  return (
    <>
      <button
        type="button"
        className={styles.btn}
        data-listening={active || undefined}
        data-preparing={preparing || undefined}
        onClick={() => (active ? stop() : start())}
        aria-label={active ? t("停止录音") : t("语音输入")}
        aria-pressed={active}
      >
        {preparing ? (
          <Loader size={17} />
        ) : listening ? (
          <Square size={17} />
        ) : (
          <AudioLines size={17} />
        )}
      </button>
      {hint && <span className={styles.hint}>{hint}</span>}
      {state === "error" && error && <span className={styles.error}>{voiceErrorText(error)}</span>}
    </>
  );
}
