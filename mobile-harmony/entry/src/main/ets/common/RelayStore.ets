import { preferences } from '@kit.ArkData';
import { common } from '@kit.AbilityKit';

/// Persisted pairing state — the relay origin + the raw pairing secret scanned
/// from the desktop QR (`https://<host>/#k=<secret>`, see
/// claw-fleet-core::mobile_relay::pairing_url). The secret grants push-registration
/// rights on the channel, so it lives only in the app's private preferences.
const STORE_NAME = 'fleet_relay';
const KEY_RELAY_URL = 'relayUrl';
const KEY_SECRET = 'secret';

export interface RelayPairing {
  relayUrl: string;
  secret: string;
}

/// 扫到的是「直连主机」那张码(桌面端「移动端」面板的第二张)。壳不持久化它 ——
/// 页面自己有一本设备簿(mobile-web 的 devices.ts),壳只负责把这次扫到的原文
/// 带进页面一次。
export interface DirectHostScan {
  baseUrl: string;
  token: string;
}

/// 一次扫码的结果:配对码,还是直连码。
export interface ScanResult {
  pairing: RelayPairing | null;
  host: DirectHostScan | null;
}

/// 解析扫到的二维码。桌面端出两张码,形状都在 fragment 里:
///   * `#k=<secret>`      —— 经中转配对(fragment 之前那半是 relay origin)
///   * `#h=<addr>&t=<tok>` —— 直连一台 HTTP 主机
/// 两者都不进请求行,所以密钥/token 不会落进任何服务端日志。
export function parseScan(raw: string): ScanResult {
  const pairing = parsePairingUrl(raw);
  if (pairing !== null) {
    return { pairing: pairing, host: null };
  }
  return { pairing: null, host: parseDirectHostUrl(raw) };
}

/// 直连码:`…/#h=<encoded baseUrl>&t=<token>`。与 mobile-web 的
/// `devices.ts::parseHostParam` 认的是同一个格式(那边有冻结向量的单测)。
export function parseDirectHostUrl(raw: string): DirectHostScan | null {
  const trimmed = raw.trim();
  const hashIdx = trimmed.indexOf('#');
  if (hashIdx < 0) {
    return null;
  }
  let baseUrl = '';
  let token = '';
  for (const part of trimmed.substring(hashIdx + 1).split('&')) {
    if (part.startsWith('h=')) {
      baseUrl = decodeURIComponent(part.substring(2));
    } else if (part.startsWith('t=')) {
      token = decodeURIComponent(part.substring(2));
    }
  }
  // 只认绝对 https/http —— 手机上那个页面是 https 的,明文 http 会被浏览器拦掉
  // (localhost 例外,但那台手机也访问不到)。
  if (!baseUrl.startsWith('https://') && !baseUrl.startsWith('http://')) {
    return null;
  }
  return { baseUrl: baseUrl, token: token };
}

/// Parse the desktop pairing URL. The secret rides in the URL fragment
/// (`#k=<secret>`) so it never reaches the relay's server logs; the part before
/// the `#` is the relay origin the WebSocket connects to.
export function parsePairingUrl(raw: string): RelayPairing | null {
  const trimmed = raw.trim();
  const hashIdx = trimmed.indexOf('#');
  if (hashIdx < 0) {
    return null;
  }
  const origin = trimmed.substring(0, hashIdx).replace(/\/+$/, '');
  const fragment = trimmed.substring(hashIdx + 1);
  let secret = '';
  for (const part of fragment.split('&')) {
    if (part.startsWith('k=')) {
      secret = part.substring(2);
      break;
    }
  }
  // 64-hex secret; the >=16 guard mirrors the relay's MIN_SECRET_LEN.
  if (origin.length === 0 || secret.length < 16) {
    return null;
  }
  return { relayUrl: origin, secret };
}

export class RelayStore {
  private static cached: preferences.Preferences | null = null;

  private static async prefs(context: common.UIAbilityContext): Promise<preferences.Preferences> {
    if (RelayStore.cached === null) {
      RelayStore.cached = await preferences.getPreferences(context, { name: STORE_NAME });
    }
    return RelayStore.cached;
  }

  static async savePairing(context: common.UIAbilityContext, pairing: RelayPairing): Promise<void> {
    const p = await RelayStore.prefs(context);
    await p.put(KEY_RELAY_URL, pairing.relayUrl);
    await p.put(KEY_SECRET, pairing.secret);
    await p.flush();
  }

  static async loadPairing(context: common.UIAbilityContext): Promise<RelayPairing | null> {
    const p = await RelayStore.prefs(context);
    const relayUrl = await p.get(KEY_RELAY_URL, '') as string;
    const secret = await p.get(KEY_SECRET, '') as string;
    if (relayUrl.length === 0 || secret.length === 0) {
      return null;
    }
    return { relayUrl, secret };
  }

  static async clearPairing(context: common.UIAbilityContext): Promise<void> {
    const p = await RelayStore.prefs(context);
    await p.delete(KEY_RELAY_URL);
    await p.delete(KEY_SECRET);
    await p.flush();
  }
}
