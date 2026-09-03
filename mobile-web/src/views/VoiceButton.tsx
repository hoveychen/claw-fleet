// 输入框旁边的语音按钮。挂在 AttachmentRow 里，所以新会话 sheet 和续写
// composer 两处自动都有。
//
// 交互是「长按说话」和「点按录音」的合流，因为这两种习惯都很常见，而它们能靠
// 松手时长区分开，不必让用户先选一种：
//   - 按住说、松手结束        —— 短句最快，全程一个动作
//   - 快速点一下 → 锁定继续录 —— 长内容不用一直按着，再点一下停
//   - 按住时上滑一段再松手    —— 丢弃这次
//
// 环境探测、provider 选择、识别生命周期都在 useVoiceInput 里；这里只管手势和画。

import { useRef, useState } from "react";
import { Mic, Square } from "lucide-react";
import { t } from "../i18n";
import { useVoiceInput, voiceErrorText } from "../useVoiceInput";
import styles from "./VoiceButton.module.css";

/** 松手时长 ≤ 这个数算「点按」，锁定继续录音；超过算「长按」，松手即停。 */
export const TAP_MS = 500;
/** 按住时上滑超过这么多像素就是要取消。 */
export const CANCEL_DY = 60;

export type Intent = "cancel" | "stop" | "lock";

/**
 * 松手时该干什么。
 *
 * @param dy   松手点相对按下点的纵向位移，向上为负。
 * @param heldMs 按住的时长。
 */
export function releaseIntent(dy: number, heldMs: number): Intent {
  // 上滑取消优先于一切:用户已经把手指移开按钮了,不管按了多久都是「不要」。
  if (dy <= -CANCEL_DY) return "cancel";
  return heldMs <= TAP_MS ? "lock" : "stop";
}

export function VoiceButton({
  lang = "zh-CN",
  onText,
}: {
  lang?: string;
  /** 识别出的一段文字。调用方决定怎么并进输入框。 */
  onText: (text: string) => void;
}) {
  const { state, partial, error, start, stop, cancel } = useVoiceInput(lang, onText);
  // 点按锁定:此时手指已经松开,录音还在继续,要再点一次才停。
  const [locked, setLocked] = useState(false);
  // 上滑到了取消区,松手就丢弃。只用来把按钮画成「松手取消」。
  const [willCancel, setWillCancel] = useState(false);
  const pressRef = useRef<{ y: number; at: number } | null>(null);

  // 探测没结束、或这台设备根本没有识别服务时,整个按钮不出现 —— 画一个按下去
  // 只会报错的按钮没有意义。
  if (state === "probing" || state === "unsupported") return null;

  const listening = state === "listening";

  const down = (e: React.PointerEvent) => {
    if (locked) return; // 锁定态下按下不重新开始,松手时才停
    pressRef.current = { y: e.clientY, at: Date.now() };
    setWillCancel(false);
    // 指针捕获:手指滑出按钮范围后仍然收得到 move/up,否则上滑取消一划出去
    // 就断了,松手事件也丢了,录音会一直开着。
    e.currentTarget.setPointerCapture?.(e.pointerId);
    start();
  };

  const move = (e: React.PointerEvent) => {
    const press = pressRef.current;
    if (!press) return;
    setWillCancel(e.clientY - press.y <= -CANCEL_DY);
  };

  const up = (e: React.PointerEvent) => {
    const press = pressRef.current;
    pressRef.current = null;
    setWillCancel(false);
    if (locked) {
      // 锁定态:这一下是「停止」。
      setLocked(false);
      stop();
      return;
    }
    if (!press) return;
    switch (releaseIntent(e.clientY - press.y, Date.now() - press.at)) {
      case "cancel":
        cancel();
        break;
      case "lock":
        setLocked(true);
        break;
      case "stop":
        stop();
        break;
    }
  };

  return (
    <>
      <button
        type="button"
        className={styles.btn}
        data-listening={listening || undefined}
        data-cancel={willCancel || undefined}
        onPointerDown={down}
        onPointerMove={move}
        onPointerUp={up}
        // 指针被系统抢走(来电、手势返回)等同于取消,别把录音留在开着的状态。
        onPointerCancel={() => {
          pressRef.current = null;
          setWillCancel(false);
          setLocked(false);
          cancel();
        }}
        // 长按在移动端会触发文本选择和系统菜单,压住。
        onContextMenu={(e) => e.preventDefault()}
        aria-label={listening ? t("停止录音") : t("语音输入")}
        aria-pressed={listening}
      >
        {listening ? <Square size={13} /> : <Mic size={13} />}
        {listening ? (willCancel ? t("松开取消") : locked ? t("点击停止") : t("松开结束")) : t("语音")}
      </button>
      {/* 实时回显。没有它用户不知道有没有听进去,只能对着一个按钮干说。 */}
      {listening && partial && <span className={styles.partial}>{partial}</span>}
      {state === "error" && error && <span className={styles.error}>{voiceErrorText(error)}</span>}
    </>
  );
}
