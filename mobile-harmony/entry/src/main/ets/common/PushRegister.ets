import { pushService, serviceNotification } from '@kit.PushKit';
import { authentication } from '@kit.AccountKit';
import { BusinessError } from '@kit.BasicServicesKit';
import { common } from '@kit.AbilityKit';
import { hilog } from '@kit.PerformanceAnalysisKit';
import { deriveChannelToken } from './Hkdf';
import { registerOpenId } from './RelayClient';
import { RelayStore } from './RelayStore';

/// Drives the full push-registration flow for the atomic service:
///   1. getToken()  — observed precondition of the verified flow (the token
///      itself is unused: account subscriptions deliver by OpenID, not token).
///   2. requestSubscribeNotification() — account subscription (华为账号订阅);
///      must pass SUBSCRIBE_WITH_HUAWEI_ID, and runs when the UI is ready.
///   3. Account Kit — silent OpenID (the account-subscription delivery target).
///   4. Derive the relay channel token from the pairing secret and report the
///      OpenID to the relay so the desktop's `notify` can reach this account.

const DOMAIN = 0x0000;
const TAG = 'fleet';
/// AGC 订阅模板「工作事项提醒」(category=WORK)，已领用审批。
const TEMPLATE_ID = '1BAAD76B2A818700';

export type RegisterState = 'unpaired' | 'subscribed';

/// getToken is a precondition observed in the on-device verified flow; its
/// failure is non-fatal to account subscription, so we log and continue.
async function ensurePushToken(): Promise<void> {
  try {
    const token = await pushService.getToken();
    hilog.info(DOMAIN, TAG, 'push token acquired (len=%{public}d)', token.length);
  } catch (e) {
    const err = e as BusinessError;
    hilog.warn(DOMAIN, TAG, 'getToken failed: %{public}d %{public}s', err.code, err.message);
  }
}

/// Account subscription. Opens the authorization sheet on first run and resolves
/// silently thereafter. SUBSCRIBE_WITH_HUAWEI_ID (=1) is mandatory for an atomic
/// service — the default (0, app token subscription) errors with 1000900022.
async function subscribeNotification(context: common.UIAbilityContext): Promise<void> {
  const result = await serviceNotification.requestSubscribeNotification(
    context,
    [TEMPLATE_ID],
    serviceNotification.SubscribeNotificationType.SUBSCRIBE_WITH_HUAWEI_ID,
  );
  hilog.info(DOMAIN, TAG, 'subscribe result: %{public}s', JSON.stringify(result));
}

/// Silent OpenID via Account Kit. `openid` scope does not prompt; the value
/// arrives in `data.data.openID`. The AuthenticationController must be created
/// with no arguments (passing a context errors with 401).
function fetchOpenId(): Promise<string> {
  return new Promise<string>((resolve, reject) => {
    const request = new authentication.HuaweiIDProvider().createAuthorizationWithHuaweiIDRequest();
    request.scopes = ['openid'];
    request.forceAuthorization = false;
    request.state = 'fleet-openid';
    const controller = new authentication.AuthenticationController();
    controller.executeRequest(request,
      (err: BusinessError<Record<string, Object>>, data: authentication.AuthenticationResponse) => {
        if (err) {
          reject(new Error('openid auth failed: ' + err.code));
          return;
        }
        const response = data as authentication.AuthorizationWithHuaweiIDResponse;
        const openId = response.data?.openID ?? '';
        if (openId.length === 0) {
          reject(new Error('response carried no openID'));
          return;
        }
        resolve(openId);
      });
  });
}

/// Full registration. Returns `'unpaired'` when no pairing secret is stored yet
/// (the caller should prompt a scan); otherwise runs the flow and returns
/// `'subscribed'`, or throws on a hard failure.
export async function registerForPush(context: common.UIAbilityContext): Promise<RegisterState> {
  const pairing = await RelayStore.loadPairing(context);
  if (pairing === null) {
    return 'unpaired';
  }
  await ensurePushToken();
  await subscribeNotification(context);
  const openId = await fetchOpenId();
  const channelToken = await deriveChannelToken(pairing.secret);
  await registerOpenId(pairing.relayUrl, channelToken, openId);
  hilog.info(DOMAIN, TAG, 'registered openId with relay %{public}s', pairing.relayUrl);
  return 'subscribed';
}
