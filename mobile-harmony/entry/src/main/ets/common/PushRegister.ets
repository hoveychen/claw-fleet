import { pushService } from '@kit.PushKit';
import { notificationManager } from '@kit.NotificationKit';
import { BusinessError } from '@kit.BasicServicesKit';
import { common } from '@kit.AbilityKit';
import { hilog } from '@kit.PerformanceAnalysisKit';

/// 推送注册的**原生那一半**:要通知授权、取设备 token。仅此而已。
///
/// 上报给 relay 的那一半交给 web(mobile-web 的 nativePush.ts + push.ts)——
/// 它本来就握着 relay 连接、channel token 和「用户是否手动关了通知」的状态。
/// 原来这里自己开一条 WebSocket 上报 openId,得在 ArkTS 里重写 HKDF 和握手
/// (Hkdf.ets + RelayClient.ets 共 194 行),两套实现还会各自漂移。
///
/// 元服务时代走的是另一条通道:AccountKit 拿华为账号 OpenID + serviceNotification
/// 模板订阅,relay 发到 `v1/{projectId}/service_notification/send`。普通应用没有
/// 账号订阅这回事,改成设备 token + `v3/{projectId}/messages:send`——也正因如此,
/// 安卓壳能和鸿蒙共用 relay 侧同一套实现。
const DOMAIN = 0x0000;
const TAG = 'fleet';

/// 通知授权。首次弹系统弹窗,之后静默返回。已授权时直接跳过,免得每次冷启动
/// 都去问一遍。用户在系统设置里关掉的话,requestEnableNotification 会直接失败
/// (1600004),此时 token 依然取得到,只是消息不会展示成通知——所以失败不致命,
/// 记一条日志继续。
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

/// 取设备 push token。token 由 Push Kit 按 client_id(module.json5 的 metadata)
/// 签发,拿不到最常见的原因是 client_id 没配或配错(1000900010 系列)。
///
/// 返回空串表示这台设备拿不到 token(模拟器、未登录华为账号、推送权益没开通),
/// 调用方据此跳过上报,而不是把空 token 送去 relay。
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
