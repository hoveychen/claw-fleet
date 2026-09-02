// 一帧摄像头画面读成一次配对 —— 从 PairScanner 里切出来，好让「RGBA → 解码 →
// 配对链接」这条管线能被单测钉住。
//
// 值得单独测的原因：管线接错（RGBA 分量顺序、宽高传反、传了没画过的 canvas）
// 不会抛任何错，只会**永远解不出码**。那种静默失效在真机上极难定位 —— 表现
// 就是「对着二维码举半天没反应」，跟光线不好、对焦不准分不开。

import jsQR from "jsqr";
import { type PairedLink, parsePairingLink } from "./pairingLink";

export interface FrameRead {
  /** 解出来且是一条配对链接。 */
  paired: PairedLink | null;
  /** 画面里确实有二维码 —— 只是它不是配对码（随手扫到的付款码之类）。
   *  与「什么都没扫到」分开，UI 才能说出「这个码不对」而不是干等着。 */
  sawCode: boolean;
}

export function readPairingFromFrame(
  data: Uint8ClampedArray,
  width: number,
  height: number,
): FrameRead {
  const found = jsQR(data, width, height);
  if (!found) return { paired: null, sawCode: false };
  return { paired: parsePairingLink(found.data), sawCode: true };
}
