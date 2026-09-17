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

/// QR code scanned is the "direct host" type (the second code on the desktop's
/// "Mobile" panel). Shell doesn't persist it — the page keeps its own device book
/// (mobile-web's devices.ts); shell just brings this scan's raw content into the
/// page once.
export interface DirectHostScan {
  baseUrl: string;
  token: string;
}

/// Result of one QR scan: pairing code or direct-host code.
export interface ScanResult {
  pairing: RelayPairing | null;
  host: DirectHostScan | null;
}

/// Parse a scanned QR code. Desktop produces two types, both with shape in the
/// fragment:
///   * `#k=<secret>`      —— relay-mediated pairing (part before fragment is relay origin)
///   * `#h=<addr>&t=<tok>` —— direct connect to HTTP host
/// Neither enters the request line, so secrets/tokens never reach any server logs.
export function parseScan(raw: string): ScanResult {
  const pairing = parsePairingUrl(raw);
  if (pairing !== null) {
    return { pairing: pairing, host: null };
  }
  return { pairing: null, host: parseDirectHostUrl(raw) };
}

/// Direct-host code: `…/#h=<encoded baseUrl>&t=<token>`. Same format that
/// mobile-web's `devices.ts::parseHostParam` recognizes (frozen-vector tests there).
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
  // Only accept absolute https/http — the page on phone is https, so plain http
  // gets blocked by the browser (localhost is an exception, but phones can't reach it).
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
