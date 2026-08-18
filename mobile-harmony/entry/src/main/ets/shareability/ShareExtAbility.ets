import ShareExtensionAbility from '@ohos.app.ability.ShareExtensionAbility';
import type UIExtensionContentSession from '@ohos.app.ability.UIExtensionContentSession';
import type Want from '@ohos.app.ability.Want';
import { hilog } from '@kit.PerformanceAnalysisKit';

const DOMAIN = 0x0000;
const TAG = 'fleet';

/// 系统分享的接收端。
///
/// 只在 EntryAbility 上声明 sendData skill 是不够的 —— 真机上 utd 确实登记进
/// 了 bm dump，但分享面板里依旧没有 Fleet。系统分享面板要求接收方提供一个
/// share 类型的 UIExtension；SDK 里 ShareExtensionAbility 就是为此存在的
/// （@ohos.app.ability.ShareExtensionAbility，继承 UIExtensionAbility）。
///
/// 这里只做转发：把 want 交给面板内的小页面，由它把内容写进与主应用共享的
/// preferences，然后拉起主应用。分享面板里不做完整交互 —— 用户要的是「把东西
/// 丢给 Fleet」，而不是在别人的面板里操作 Fleet。
export default class ShareExtAbility extends ShareExtensionAbility {
  onSessionCreate(want: Want, session: UIExtensionContentSession): void {
    hilog.info(DOMAIN, TAG, 'share extension session created');
    const storage: LocalStorage = new LocalStorage();
    storage.setOrCreate('shareWant', want);
    storage.setOrCreate('shareSession', session);
    session.loadContent('pages/SharePanel', storage);
  }

  onSessionDestroy(_session: UIExtensionContentSession): void {
    hilog.info(DOMAIN, TAG, 'share extension session destroyed');
  }
}
