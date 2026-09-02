// 一条配对链接怎么读 —— 三条入口共用的纯解析。
//
// 桌面端的二维码是 `https://<relay-host>/#k=<secret>`
// (claw-fleet-core::mobile_relay::pairing_url)。同一个形状会从三个地方进来:
//
//   - PWA:被这个 URL 打开,`window.location.hash` 就是它(devices.ts)
//   - 原生壳:App Link / Universal Link 把整个 URL 递进来(deepLink.ts)
//   - 手动粘贴:用户自己把链接贴进配对门(PairPasteForm.tsx)
//
// 第三条是自建 relay 的**唯一**入口:App Link 要求在 manifest 里编译期写死
// host,而自建 relay 的 host 编译期不可知,所以扫码只会打开浏览器,永远进不了
// app。这一条不依赖任何 host 声明。

import { pairingLinkRelayBase } from "./relayBase";
import { extractSecretFromUrl } from "./secretStore";

/** 一次配对递过来的东西:密钥,以及它指名的 relay(`null` = 没指名,用构建默认
 *  值)。**两样都要**——只取密钥的话,扫了自建 relay 的二维码,app 照样去连打包
 *  时烧进去的官方 relay,而现象只是「一直连不上」。 */
export interface PairedLink {
  secret: string;
  relayBase: string | null;
}

/** relay 对 auth 帧的最低长度要求(fleet-relay/src/ws.rs `MIN_SECRET_LEN`)。
 *  桌面端生成的是 64 位十六进制,所以这道门只拦明显不是密钥的输入 —— 让用户
 *  当场知道「这条链接不对」,而不是配上之后卡在一个连不上的通道里。 */
const MIN_SECRET_LEN = 16;

/** 解析一条完整的配对链接。`null` = 这不是一条可用的配对链接。 */
export function parsePairingLink(raw: string): PairedLink | null {
  const url = raw.trim();
  if (!url) return null;
  const secret = extractSecretFromUrl(url);
  if (!secret || secret.length < MIN_SECRET_LEN) return null;
  return { secret, relayBase: pairingLinkRelayBase(url) };
}
