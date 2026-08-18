import { preferences } from '@kit.ArkData';
import { common } from '@kit.AbilityKit';
import { util } from '@kit.ArkTS';

/// Device identity fed into the relay `client_hello` heartbeat — the native peer
/// of mobile-web's clientId.ts + deviceLabel.ts.
///
///   - clientId: a stable-per-install UUID so the desktop can dedup this phone's
///     repeated heartbeats into one "connected device" row. Persisted in the
///     app's private preferences; if wiped, a fresh id is minted and the phone
///     simply shows up as a new device.
///   - label / platform: unlike the web (where the UA is all we get), the atomic
///     service can read the real device name via @kit.BasicServicesKit. The
///     platform key is always 'harmony' so the desktop picks the right icon.

const STORE_NAME = 'fleet_device';
const KEY_CLIENT_ID = 'clientId';

export const DEVICE_PLATFORM = 'harmony';

let cachedStore: preferences.Preferences | null = null;

async function store(context: common.UIAbilityContext): Promise<preferences.Preferences> {
  if (cachedStore === null) {
    cachedStore = await preferences.getPreferences(context, { name: STORE_NAME });
  }
  return cachedStore;
}

/// Return this install's client id, generating and persisting one on first use.
export async function loadClientId(context: common.UIAbilityContext): Promise<string> {
  const p = await store(context);
  const existing = await p.get(KEY_CLIENT_ID, '') as string;
  if (existing.length > 0) {
    return existing;
  }
  const id = util.generateRandomUUID(true);
  await p.put(KEY_CLIENT_ID, id);
  await p.flush();
  return id;
}

/// Human label shown in the desktop's connected-device list. `deviceInfo`'s
/// model fields (marketName / productModel) are NOT available to an atomic
/// service (compile error 11706010), so we announce a plain platform label —
/// the desktop already renders a HarmonyOS icon off the `platform` key.
export function deviceLabel(): string {
  return 'HarmonyOS 元服务';
}
