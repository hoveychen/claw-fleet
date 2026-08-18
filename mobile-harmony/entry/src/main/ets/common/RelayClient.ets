import { webSocket } from '@kit.NetworkKit';
import { BusinessError } from '@kit.BasicServicesKit';

/// One-shot registration of this Huawei account's OpenID for push on the paired
/// relay channel.
///
/// The relay stores subscriptions server-side (per channel, on disk), so we do
/// not keep a persistent socket: connect, `auth` as a client with the derived
/// channel token, send one `push_subscribe` frame carrying the OpenID, then
/// close. Push notifications themselves arrive out-of-band via Huawei Push Kit,
/// never over this socket — so there is nothing to stay connected for.
///
/// The frames are the relay's plaintext envelope frames (see
/// fleet-relay/src/frames.rs); only `msg` payloads are end-to-end encrypted, and
/// this flow sends none, so no AES-GCM layer is needed here.

const CONNECT_TIMEOUT_MS = 10000;
/// The relay has no ack for `push_subscribe`; give it a beat to persist before
/// we drop the socket.
const PERSIST_GRACE_MS = 500;

interface RelayFrame {
  type?: string;
  message?: string;
}

interface AuthFrame {
  type: string;
  role: string;
  secret: string;
}

interface HarmonySubscription {
  platform: string;
  openId: string;
}

interface PushSubscribeFrame {
  type: string;
  subscription: HarmonySubscription;
}

function toWsUrl(relayUrl: string): string {
  let base = relayUrl.replace(/\/+$/, '');
  if (base.startsWith('https://')) {
    base = 'wss://' + base.substring('https://'.length);
  } else if (base.startsWith('http://')) {
    base = 'ws://' + base.substring('http://'.length);
  }
  return base + '/ws';
}

export function registerOpenId(relayUrl: string, channelToken: string, openId: string): Promise<void> {
  return new Promise<void>((resolve, reject) => {
    const ws = webSocket.createWebSocket();
    let settled = false;

    const finish = (err: string | null): void => {
      if (settled) {
        return;
      }
      settled = true;
      ws.close((_e: BusinessError, _ok: boolean) => {});
      if (err !== null) {
        reject(new Error(err));
      } else {
        resolve();
      }
    };

    ws.on('open', (_err: BusinessError, _value: Object) => {
      const auth: AuthFrame = { type: 'auth', role: 'client', secret: channelToken };
      ws.send(JSON.stringify(auth), (_e: BusinessError, _ok: boolean) => {});
    });

    ws.on('message', (_err: BusinessError, value: string | ArrayBuffer) => {
      if (typeof value !== 'string') {
        return;
      }
      let frame: RelayFrame;
      try {
        frame = JSON.parse(value) as RelayFrame;
      } catch (e) {
        return; // ignore non-JSON
      }
      if (frame.type === 'authed') {
        const subscription: HarmonySubscription = { platform: 'harmony', openId: openId };
        const sub: PushSubscribeFrame = { type: 'push_subscribe', subscription: subscription };
        ws.send(JSON.stringify(sub), (_e: BusinessError, _ok: boolean) => {
          setTimeout(() => finish(null), PERSIST_GRACE_MS);
        });
      } else if (frame.type === 'error') {
        finish('relay rejected: ' + (frame.message ?? 'unknown'));
      }
    });

    ws.on('error', (err: BusinessError) => {
      finish('websocket error: ' + JSON.stringify(err));
    });

    ws.connect(toWsUrl(relayUrl), (err: BusinessError, _ok: boolean) => {
      if (err) {
        finish('connect failed: ' + JSON.stringify(err));
      }
    });

    setTimeout(() => finish('timed out waiting for relay'), CONNECT_TIMEOUT_MS);
  });
}
