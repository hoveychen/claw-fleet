import { common } from '@kit.AbilityKit';
import { BusinessError } from '@kit.BasicServicesKit';
import { hilog } from '@kit.PerformanceAnalysisKit';
import { scanBarcode, scanCore } from '@kit.ScanKit';
import { parsePairingUrl, RelayStore } from '../common/RelayStore';
import { registerForPush } from '../common/PushRegister';
import { ColorTokens, themeTokens } from '../common/Theme';
import { t } from '../common/I18n';

const DOMAIN = 0x0000;
const TAG = 'fleet';

/// The unpaired gate: scan the desktop QR, persist the pairing, subscribe to
/// push, then hand control to the shell via `onPaired`. Mirrors mobile-web's
/// pairing gate; the push-registration flow is the device-verified P5 path.
@Component
export struct GateView {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  @StorageProp('lang') lang: string = 'zh';
  onPaired: () => void = () => {};
  // Enter the no-scan trial mode (simulated data). Lets the atomic service be
  // used and reviewed standalone before a desktop pairing QR is scanned.
  onTrial: () => void = () => {};
  @State status: string = '';
  @State busy: boolean = false;

  private tk(): ColorTokens {
    return themeTokens(this.resolvedTheme);
  }

  private ctx(): common.UIAbilityContext {
    return this.getUIContext().getHostContext() as common.UIAbilityContext;
  }

  aboutToAppear(): void {
    this.status = t('尚未配对。请扫描 Fleet 桌面端「移动端」页的二维码。');
  }

  private async startPairing(): Promise<void> {
    if (this.busy) {
      return;
    }
    try {
      const options: scanBarcode.ScanOptions = {
        scanTypes: [scanCore.ScanType.QR_CODE],
        enableMultiMode: false,
        enableAlbum: true,
      };
      const result = await scanBarcode.startScanForResult(this.ctx(), options);
      const pairing = parsePairingUrl(result.originalValue);
      if (pairing === null) {
        this.status = t('二维码无法识别为 Fleet 配对码。');
        return;
      }
      await RelayStore.savePairing(this.ctx(), pairing);
      this.busy = true;
      this.status = t('配对成功，正在订阅推送…');
      await registerForPush(this.ctx());
      this.busy = false;
      this.onPaired();
    } catch (e) {
      const err = e as BusinessError;
      hilog.error(DOMAIN, TAG, 'pairing failed: %{public}s', JSON.stringify(err));
      this.status = t('订阅失败：{0}', err.message ?? JSON.stringify(err));
      this.busy = false;
    }
  }

  build() {
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
      Text(t('Fleet 移动端'))
        .fontSize(24)
        .fontWeight(FontWeight.Bold)
        .fontColor(this.tk().text)
      Text(this.status)
        .fontSize(15)
        .fontColor(this.tk().textSecondary)
        .textAlign(TextAlign.Center)
        .lineHeight(22)
      Button(t('扫码配对'))
        .width('72%')
        .enabled(!this.busy)
        .backgroundColor(this.tk().accent)
        .onClick(() => {
          this.startPairing();
        })
      Text(t('免扫码体验(试用模式)'))
        .fontSize(14)
        .fontColor(this.tk().accent)
        .padding({ top: 4, bottom: 4 })
        .enabled(!this.busy)
        .onClick(() => {
          this.onTrial();
        })
    }
    .width('100%')
    .height('100%')
    .justifyContent(FlexAlign.Center)
    .padding({ left: 24, right: 24 })
    .backgroundColor(this.tk().bg)
  }
}
