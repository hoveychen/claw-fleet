import { common } from '@kit.AbilityKit';
import { hilog } from '@kit.PerformanceAnalysisKit';
import { RelayStore } from '../common/RelayStore';
import { registerForPush } from '../common/PushRegister';
import { ColorTokens, themeTokens } from '../common/Theme';
import { handleBack } from '../common/BackStack';
import { t } from '../common/I18n';
import { GateView } from '../views/GateView';
import { MainShell } from '../views/MainShell';

const DOMAIN = 0x0000;
const TAG = 'fleet';

/// App root: decides between the pairing gate and the main shell. On a paired
/// launch it refreshes the push subscription in the background (the P5 flow).
@Entry
@Component
struct Index {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  @State private paired: boolean = false;
  @State private checking: boolean = true;
  // No-scan trial mode: the shell runs against MockRelayClient's simulated data
  // instead of a live relay. Entered from the gate's 「免扫码体验」 button; exited
  // via the 更多 page. Independent of `paired` (trial needs no pairing secret).
  @State private trial: boolean = false;
  // Relay auth failure on a paired launch (secret reset on the desktop). Mirrors
  // mobile-web's authError page: an explicit "清除本机密钥" exit rather than a
  // dead status string.
  @State private authError: string = '';

  private ctx(): common.UIAbilityContext {
    return this.getUIContext().getHostContext() as common.UIAbilityContext;
  }

  private tk(): ColorTokens {
    return themeTokens(this.resolvedTheme);
  }

  aboutToAppear(): void {
    this.init();
  }

  /// Route the system back event (hardware key / navigation gesture) through the
  /// shared back stack. true = consumed (overlay closed / tab switched / exit
  /// armed); false = let the platform background or exit the app.
  onBackPress(): boolean {
    return handleBack();
  }

  private async init(): Promise<void> {
    try {
      const pairing = await RelayStore.loadPairing(this.ctx());
      this.paired = pairing !== null;
    } catch (e) {
      this.paired = false;
    }
    this.checking = false;
    if (this.paired) {
      // Refresh the push subscription on a paired launch (fire-and-forget) —
      // preserves the on-device-verified P5 flow (getToken → subscribe → OpenID).
      registerForPush(this.ctx())
        .then((state: string) => {
          // Surface the subscription state so the relay client_hello can report
          // pushSubscribed to the desktop (drives the push badge on the device row).
          AppStorage.setOrCreate('pushSubscribed', state === 'subscribed');
          hilog.info(DOMAIN, TAG, 'push refreshed on launch: %{public}s', state);
        })
        .catch((e: Object) => {
          hilog.warn(DOMAIN, TAG, 'push refresh failed: %{public}s', JSON.stringify(e));
        });
    }
  }

  private async doRepair(): Promise<void> {
    await RelayStore.clearPairing(this.ctx());
    this.authError = '';
    this.paired = false;
  }

  /// Paired-but-auth-failed page (secret reset on the desktop). Mirrors
  /// mobile-web's authError gate: a clear-secret button, not a dead status line.
  @Builder
  authErrorPage() {
    Column({ space: 20 }) {
      Text('F')
        .fontSize(40)
        .fontWeight(FontWeight.Bold)
        .fontColor('#ffffff')
        .width(76)
        .height(76)
        .borderRadius(19)
        .backgroundColor(this.tk().accent)
        .textAlign(TextAlign.Center)
      Text(t('配对失败'))
        .fontSize(24)
        .fontWeight(FontWeight.Bold)
        .fontColor(this.tk().text)
      Text(t('{0}。密钥可能已被重置，请回到桌面端重新扫码。', this.authError))
        .fontSize(15)
        .fontColor(this.tk().textSecondary)
        .textAlign(TextAlign.Center)
        .lineHeight(22)
      Button(t('清除本机密钥'))
        .width('72%')
        .backgroundColor(this.tk().accent)
        .onClick(() => {
          this.doRepair();
        })
    }
    .width('100%')
    .height('100%')
    .justifyContent(FlexAlign.Center)
    .padding({ left: 24, right: 24 })
    .backgroundColor(this.tk().bg)
  }

  build() {
    Stack() {
      if (this.checking) {
        Column() {
          Text('Fleet')
            .fontSize(22)
            .fontWeight(FontWeight.Bold)
            .fontColor(this.tk().text)
        }
        .width('100%')
        .height('100%')
        .justifyContent(FlexAlign.Center)
        .backgroundColor(this.tk().bg)
      } else if (!this.paired && !this.trial) {
        GateView({
          onPaired: () => {
            this.authError = '';
            this.paired = true;
          },
          onTrial: () => {
            this.trial = true;
          },
        })
      } else if (this.authError.length > 0) {
        this.authErrorPage()
      } else {
        MainShell({
          trial: this.trial,
          onRepair: () => {
            // In trial mode there is no pairing secret to clear — 退出试用 just
            // returns to the gate; a real pairing clears the stored secret.
            if (this.trial) {
              this.trial = false;
            } else {
              this.doRepair();
            }
          },
          onNewSession: () => {
            // New-session composer lands in P6.
          },
          onAuthError: (message: string) => {
            this.authError = message;
          },
        })
      }
    }
    .width('100%')
    .height('100%')
  }
}
