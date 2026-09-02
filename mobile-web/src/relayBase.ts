// 「这台设备的 relay 在哪」—— 纯粹的 URL 归属计算,不含任何 relay 客户端。
//
// 这些函数原先住在 relay.ts,而 relay.ts 是**整个 relay 客户端**(WebSocket、
// 端到端加密、帧协议)。谁想知道一个 relay 地址,就得把那一整棵树拖进来:同源
// (webui)构建为此专门用动态 import 绕路(见 main.tsx 与 push.ts 的注释)。
// 多设备之后需要知道地址的地方多了(设备簿要记住扫码指名的 relay、推送要按
// relay 取 VAPID 公钥),所以这一小块被切出来单独住。
//
// 本模块**没有模块加载期副作用**:不读 hash、不读 env、不建任何东西。原先
// relay.ts 里那个 `const RELAY_BASE = resolveRelayBase(...)` 之所以必须在模块
// 加载时求值,是因为配对 fragment 会在启动后立刻被抹掉;现在扫码指名的 relay
// 在落地那一刻就存进了设备记录(devices.ts),没有什么需要抢在抹掉之前读了。

/** 二维码 fragment 里的 `&relay=<encoded origin>`,解析成一个 origin。
 *
 *  只认绝对的 http/https URL —— 二维码是不可信输入,而这个值会成为客户端后续
 *  每一个 URL 的基底。取 origin(丢掉 path、规范化尾斜杠):带路径前缀的 relay
 *  在这个客户端里本来就不被支持(PWA 的基底取自 window.location.origin,同样
 *  会丢掉前缀)。
 *
 *  只有鸿蒙壳会写这个参数(WebShell.ets → RelayStore):它的页面 origin 是假的
 *  `https://fleet.local`,没有这个参数就只能一直拨打包时烧进去的那个 relay,
 *  自建 relay 永远配不上。 */
export function parseRelayParam(hash: string): string | null {
  const match = hash.match(/[#&]relay=([^&]+)/);
  if (!match) return null;
  let candidate: URL;
  try {
    candidate = new URL(decodeURIComponent(match[1]));
  } catch {
    return null;
  }
  if (candidate.protocol !== "https:" && candidate.protocol !== "http:") return null;
  return candidate.origin;
}

/** 没有指名 relay 的设备用哪个地址。
 *
 *  `baked` 是 `VITE_RELAY_URL`,构建时烧进去(鸿蒙壳经
 *  `mobile-harmony/scripts/sync-web.sh` 烧;它的页面 origin 是假域名,退回
 *  origin 会让 app 拨打自己)。`origin` 是 `window.location.origin` —— PWA 的
 *  正解,因为那份页面正是 relay 自己发出来的。
 *
 *  取参数而非直接读全局,好让它保持纯函数可测。 */
export function defaultRelayBaseFrom(baked: string | undefined, origin: string): string {
  return baked || origin;
}

/** 上面那个的活体版本:构建常量 + 当前 origin。 */
export function defaultRelayBase(): string {
  return defaultRelayBaseFrom(import.meta.env.VITE_RELAY_URL, window.location.origin);
}

/** 一台设备实际连的 relay:它自己指名的那个,否则构建默认值。 */
export function relayBaseFor(relayBase: string | null | undefined): string {
  return relayBase ?? defaultRelayBase();
}

/** 三个能指名 relay 的来源合成一个地址。纯函数,好让它脱离 `window` 可测。
 *
 *  `hash` 里的 `&relay=` 胜过 `baked`:前者描述的是**这一次配对**,后者只是这
 *  份包恰好带的默认值。 */
export function resolveRelayBase(
  hash: string,
  baked: string | undefined,
  origin: string,
): string {
  return parseRelayParam(hash) ?? defaultRelayBaseFrom(baked, origin);
}

/** relay 主机名的简短人类可读形式,给「更多」页显示一行。`https` 是常态所以
 *  它的 scheme 作为噪音被丢掉;其他(比如 `http://127.0.0.1:…` 的开发 relay)
 *  保留 scheme —— 那个差别恰恰是你看这一行想知道的东西。 */
export function relayDisplayHost(base: string): string {
  try {
    const u = new URL(base);
    return u.protocol === "https:" ? u.host : `${u.protocol}//${u.host}`;
  } catch {
    return base;
  }
}

/** 该 relay 的 WebSocket 端点。 */
export function relayWsUrl(base: string): string {
  return base.replace(/\/$/, "").replace(/^http/, "ws") + "/ws";
}
