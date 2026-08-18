import { common } from '@kit.AbilityKit';
import { promptAction } from '@kit.ArkUI';
import { t, setLang } from '../common/I18n';
import { ColorTokens, themeTokens, setThemeMode, ThemeMode } from '../common/Theme';
import { getKeepScreenOn, setKeepScreenOn } from '../common/WakeLock';
import { registerForPush } from '../common/PushRegister';
import { Icon } from '../common/Icon';

/// The "更多" tab: a 工具 nav card, a 设置 card (语言 / 主题 / 屏幕常亮), the live
/// 连接与通知 card, a red danger re-pair row with a confirm dialog, and the about
/// block. Mirrors mobile-web's MoreView; the screen-keep-awake toggle uses the
/// native window API (see WakeLock.ets) and the push row surfaces denied/unpaired
/// states as explanatory notes (the native equivalent of the web's push multi-state).
@Component
export struct MoreView {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  @StorageProp('lang') lang: string = 'zh';
  @StorageProp('themeMode') themeMode: string = 'system';
  @StorageProp('keepScreenOn') keepScreenOn: boolean = false;
  @Prop connected: boolean = false;
  @Prop agentOnline: boolean = false;
  // No-scan trial mode: swaps the 桌面端 status to 「试用模式」 and the 配对 danger
  // row to a 「退出试用」 action (no secret to clear).
  @Prop trial: boolean = false;
  @StorageProp('pushSubscribed') pushSubscribed: boolean = false;
  @State private enablingPush: boolean = false;
  @State private pushNote: string = '';
  onRepair: () => void = () => {};
  onOpenUsage: () => void = () => {};
  onOpenRepo: () => void = () => {};

  private tk(): ColorTokens {
    return themeTokens(this.resolvedTheme);
  }

  private ctx(): common.UIAbilityContext {
    return this.getUIContext().getHostContext() as common.UIAbilityContext;
  }

  private enablePush(): void {
    if (this.enablingPush) {
      return;
    }
    this.enablingPush = true;
    this.pushNote = '';
    registerForPush(this.ctx())
      .then((state: string) => {
        const subscribed = state === 'subscribed';
        AppStorage.setOrCreate('pushSubscribed', subscribed);
        if (!subscribed) {
          // 'unpaired' — the account subscription can't run before pairing.
          this.pushNote = t('尚未配对，请先扫码配对后再开启通知。');
          promptAction.showToast({ message: t('尚未配对。请先扫码配对。') });
        }
      })
      .catch((e: Object) => {
        // A hard subscribe/OpenID failure — the native equivalent of the web's
        // "denied" note: surface it inline instead of silently swallowing it.
        const msg = e instanceof Error ? (e as Error).message : `${e}`;
        this.pushNote = t('订阅推送失败，请稍后重试。');
        promptAction.showToast({ message: t('订阅失败：{0}', msg) });
      })
      .finally(() => {
        this.enablingPush = false;
      });
  }

  private confirmRepair(): void {
    AlertDialog.show({
      message: t('清除本机配对密钥？需回到桌面端重新扫码才能再连接。'),
      autoCancel: true,
      alignment: DialogAlignment.Center,
      primaryButton: {
        value: t('取消'),
        action: () => {},
      },
      secondaryButton: {
        value: t('确定'),
        fontColor: this.tk().error,
        action: () => this.onRepair(),
      },
    });
  }

  private connLabel(): string {
    if (this.trial) {
      return t('试用模式');
    }
    if (!this.connected) {
      return t('连接中…');
    }
    return this.agentOnline ? t('桌面端在线') : t('桌面端离线');
  }

  private connColor(): string {
    if (this.trial) {
      return this.tk().accent;
    }
    if (!this.connected) {
      return this.tk().textDim;
    }
    return this.agentOnline ? this.tk().success : this.tk().warning;
  }

  @Builder
  sectionTitle(title: string) {
    Text(title)
      .fontSize(11)
      .fontWeight(FontWeight.Medium)
      .fontColor(this.tk().textDim)
      .width('100%')
      .padding({ left: 4 })
      .margin({ top: 20, bottom: 7 })
  }

  @Builder
  cardDivider() {
    Divider()
      .color(this.tk().border)
      .margin({ left: 14, right: 14 })
  }

  /// Tool nav row (icon accent-coloured, label + sub, trailing chevron).
  @Builder
  navRow(icon: Resource, label: string, sub: string, onTap: () => void) {
    Row({ space: 12 }) {
      Icon({ src: icon, iconSize: 18, color: this.tk().accent })
      Column({ space: 2 }) {
        Text(label).fontSize(14).fontColor(this.tk().text)
        Text(sub).fontSize(11).fontColor(this.tk().textDim).maxLines(1)
          .textOverflow({ overflow: TextOverflow.Ellipsis })
      }
      .alignItems(HorizontalAlign.Start)
      .layoutWeight(1)
      Icon({ src: $r('app.media.ic_chevron_right'), iconSize: 18, color: this.tk().textDim })
    }
    .width('100%')
    .padding({ left: 14, right: 14, top: 12, bottom: 12 })
    .constraintSize({ minHeight: 46 })
    .onClick(onTap)
  }

  /// Whether the (group, value) segment is the selected one. Reads the live
  /// @StorageProp *inside* the builder's render scope so ArkUI subscribes to it —
  /// passing a precomputed `active` boolean by value into the @Builder would NOT
  /// re-render the pill when the setting changes (the highlight would freeze even
  /// though the setting took effect). Mirrors TasksView.segButton's `this.markFilter === key`.
  private segActive(group: string, value: string): boolean {
    if (group === 'lang') {
      return (this.lang === 'en' ? 'en' : 'zh') === value;
    }
    if (group === 'theme') {
      return this.themeMode === value;
    }
    // 'wake'
    return (this.keepScreenOn ? 'on' : 'off') === value;
  }

  @Builder
  segButton(group: string, value: string, label: string, onTap: () => void) {
    Text(label)
      .fontSize(13)
      .fontWeight(this.segActive(group, value) ? 600 : FontWeight.Normal)
      .fontColor(this.segActive(group, value) ? this.tk().accent : this.tk().textSecondary)
      .backgroundColor(this.segActive(group, value) ? this.tk().accentSoft : Color.Transparent)
      .padding({ top: 5, bottom: 5, left: 13, right: 13 })
      .borderRadius(6)
      .onClick(onTap)
  }

  /// A 设置-card row: label on the left, segmented control (2–3 buttons) on the
  /// right. Row spacing/padding matches the web `.row` + `.segment` pair.
  @Builder
  segShellStart(label: string) {
    Text(label).fontSize(14).fontColor(this.tk().text).flexShrink(0)
  }

  @Builder
  langRow() {
    Row() {
      this.segShellStart(t('语言'))
      Row({ space: 4 }) {
        this.segButton('lang', 'zh', '中文', () => setLang('zh'))
        this.segButton('lang', 'en', 'English', () => setLang('en'))
      }
      .padding(3)
      .backgroundColor(this.tk().bgSecondary)
      .border({ width: 1, color: this.tk().border })
      .borderRadius(9)
    }
    .width('100%')
    .justifyContent(FlexAlign.SpaceBetween)
    .padding({ left: 14, right: 14, top: 12, bottom: 12 })
    .constraintSize({ minHeight: 46 })
  }

  @Builder
  themeRow() {
    Row() {
      this.segShellStart(t('主题'))
      Row({ space: 4 }) {
        this.segButton('theme', 'system', t('跟随系统'), () => setThemeMode('system' as ThemeMode))
        this.segButton('theme', 'light', t('亮色'), () => setThemeMode('light' as ThemeMode))
        this.segButton('theme', 'dark', t('暗色'), () => setThemeMode('dark' as ThemeMode))
      }
      .padding(3)
      .backgroundColor(this.tk().bgSecondary)
      .border({ width: 1, color: this.tk().border })
      .borderRadius(9)
    }
    .width('100%')
    .justifyContent(FlexAlign.SpaceBetween)
    .padding({ left: 14, right: 14, top: 12, bottom: 12 })
    .constraintSize({ minHeight: 46 })
  }

  @Builder
  wakeRow() {
    Row() {
      this.segShellStart(t('屏幕常亮'))
      Row({ space: 4 }) {
        this.segButton('wake', 'off', t('关'), () => setKeepScreenOn(this.ctx(), false))
        this.segButton('wake', 'on', t('开'), () => setKeepScreenOn(this.ctx(), true))
      }
      .padding(3)
      .backgroundColor(this.tk().bgSecondary)
      .border({ width: 1, color: this.tk().border })
      .borderRadius(9)
    }
    .width('100%')
    .justifyContent(FlexAlign.SpaceBetween)
    .padding({ left: 14, right: 14, top: 12, bottom: 12 })
    .constraintSize({ minHeight: 46 })
  }

  build() {
    Scroll() {
      Column() {
        Text(t('更多'))
          .fontSize(22)
          .fontWeight(FontWeight.Bold)
          .fontColor(this.tk().text)
          .width('100%')

        // ── 工具 ──
        this.sectionTitle(t('工具'))
        Column() {
          this.navRow($r('app.media.ic_folder_git_2'), t('仓库'), t('查看未合并 worktree 与未推提交'), () => this.onOpenRepo())
          this.cardDivider()
          this.navRow($r('app.media.ic_gauge'), t('账号与用量'), t('今日花费、账号档案与限流占用'), () => this.onOpenUsage())
        }
        .width('100%')
        .backgroundColor(this.tk().bgCard)
        .border({ width: 1, color: this.tk().border })
        .borderRadius(12)
        .clip(true)

        // ── 设置 ──
        this.sectionTitle(t('设置'))
        Column() {
          this.langRow()
          this.cardDivider()
          this.themeRow()
          this.cardDivider()
          this.wakeRow()
        }
        .width('100%')
        .backgroundColor(this.tk().bgCard)
        .border({ width: 1, color: this.tk().border })
        .borderRadius(12)
        .clip(true)

        // ── 连接与通知 ──
        this.sectionTitle(t('连接与通知'))
        Column() {
          Row() {
            Text(t('桌面端')).fontSize(14).fontColor(this.tk().text).layoutWeight(1)
            Row({ space: 7 }) {
              Circle({ width: 8, height: 8 }).fill(this.connColor())
              Text(this.connLabel()).fontSize(13).fontColor(this.tk().textSecondary)
            }
          }
          .width('100%')
          .padding({ left: 14, right: 14, top: 12, bottom: 12 })
          .constraintSize({ minHeight: 46 })
          this.cardDivider()
          Row() {
            Text(t('通知')).fontSize(14).fontColor(this.tk().text).layoutWeight(1)
            if (this.pushSubscribed) {
              Text(t('已开启')).fontSize(13).fontColor(this.tk().success)
            } else {
              Text(this.enablingPush ? t('订阅中…') : t('开启'))
                .fontSize(13)
                .fontWeight(FontWeight.Medium)
                .fontColor('#ffffff')
                .backgroundColor(this.tk().accent)
                .padding({ left: 16, right: 16, top: 6, bottom: 6 })
                .borderRadius(7)
                .onClick(() => this.enablePush())
            }
          }
          .width('100%')
          .padding({ left: 14, right: 14, top: 12, bottom: 12 })
          .constraintSize({ minHeight: 46 })
          if (this.pushNote.length > 0) {
            Text(this.pushNote)
              .fontSize(12)
              .fontColor(this.tk().textSecondary)
              .lineHeight(19)
              .width('100%')
              .padding({ left: 14, right: 14, bottom: 12 })
          }
        }
        .width('100%')
        .backgroundColor(this.tk().bgCard)
        .border({ width: 1, color: this.tk().border })
        .borderRadius(12)
        .clip(true)

        // ── 配对 ──
        this.sectionTitle(t('配对'))
        Column() {
          if (this.trial) {
            Text(t('退出试用 / 扫码连接桌面端'))
              .fontSize(14)
              .fontColor(this.tk().accent)
              .width('100%')
              .textAlign(TextAlign.Start)
              .padding({ left: 14, right: 14, top: 13, bottom: 13 })
              .onClick(() => this.onRepair())
          } else {
            Text(t('重新配对 / 清除密钥'))
              .fontSize(14)
              .fontColor(this.tk().error)
              .width('100%')
              .textAlign(TextAlign.Start)
              .padding({ left: 14, right: 14, top: 13, bottom: 13 })
              .onClick(() => this.confirmRepair())
          }
        }
        .width('100%')
        .backgroundColor(this.tk().bgCard)
        .border({ width: 1, color: this.tk().border })
        .borderRadius(12)
        .clip(true)

        // ── 关于 ──
        this.sectionTitle(t('关于'))
        Column({ space: 4 }) {
          Text('Claw Fleet').fontSize(15).fontColor(this.tk().text)
          Text('HarmonyOS 元服务 · v0.1').fontSize(12).fontColor(this.tk().textDim)
        }
        .alignItems(HorizontalAlign.Start)
        .width('100%')
        .padding(14)
        .backgroundColor(this.tk().bgCard)
        .border({ width: 1, color: this.tk().border })
        .borderRadius(12)
      }
      .width('100%')
      .alignItems(HorizontalAlign.Start)
      .padding({ left: 16, right: 16, top: 16, bottom: 28 })
    }
    .width('100%')
    .height('100%')
    .align(Alignment.Top)
    .backgroundColor(this.tk().bg)
  }
}
