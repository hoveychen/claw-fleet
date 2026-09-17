import { pushService } from '@kit.PushKit';
import { notificationManager } from '@kit.NotificationKit';
import { BusinessError } from '@kit.BasicServicesKit';
import { common } from '@kit.AbilityKit';
import { hilog } from '@kit.PerformanceAnalysisKit';

/// The **native side** of push registration: ask for notification permission,
/// get the device token. That's all.
///
/// The side that reports to relay goes to the web (mobile-web's nativePush.ts +
/// push.ts) — it already holds the relay connection, channel token, and the state
/// of whether the user manually disabled notifications. Originally we opened a
/// WebSocket here to report openId, requiring HKDF and handshake rewritten in ArkTS
/// (Hkdf.ets + RelayClient.ets totaling 194 lines); two implementations drift apart.
///
/// In the atomic service era we used a different channel: AccountKit got the
/// Huawei account OpenID + serviceNotification template subscription, relay sent
/// to `v1/{projectId}/service_notification/send`. Regular apps don't have account
/// subscription, so we switched to device token + `v3/{projectId}/messages:send` —
/// that's why Android and HarmonyOS can share the same relay-side implementation.
const DOMAIN = 0x0000;
const TAG = 'fleet';

/// Request notification permission. First time pops a system dialog, subsequent
/// calls return silently. If already granted, skip to avoid asking on every cold
/// start. If the user disabled notifications in system settings, requestEnableNotification
/// fails directly (1600004), but the token is still available — messages just won't
/// show as notifications. Failure is not fatal; log and continue.
async function ensureNotificationEnabled(context: common.UIAbilityContext): Promise<void> {
  try {
    if (await notificationManager.isNotificationEnabled()) {
      return;
    }
    await notificationManager.requestEnableNotification(context);
  } catch (e) {
    const err = e as BusinessError;
    hilog.warn(DOMAIN, TAG, 'notification enable failed: %{public}d %{public}s', err.code, err.message);
  }
}

/// Get the device push token. Push Kit signs the token based on client_id
/// (in module.json5's metadata); most common failure is missing or wrong client_id
/// (1000900010 series).
///
/// Empty string return means this device can't get a token (emulator, Huawei
/// account not logged in, push service not enabled); caller should skip reporting
/// rather than send an empty token to relay.
export async function acquirePushToken(context: common.UIAbilityContext): Promise<string> {
  await ensureNotificationEnabled(context);
  try {
    const token = await pushService.getToken();
    hilog.info(DOMAIN, TAG, 'push token acquired (len=%{public}d)', token.length);
    return token;
  } catch (e) {
    const err = e as BusinessError;
    hilog.warn(DOMAIN, TAG, 'getToken failed: %{public}d %{public}s', err.code, err.message);
    return '';
  }
}
