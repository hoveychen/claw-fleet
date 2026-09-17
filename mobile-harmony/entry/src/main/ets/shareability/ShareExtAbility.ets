import ShareExtensionAbility from '@ohos.app.ability.ShareExtensionAbility';
import type UIExtensionContentSession from '@ohos.app.ability.UIExtensionContentSession';
import type Want from '@ohos.app.ability.Want';
import { hilog } from '@kit.PerformanceAnalysisKit';

const DOMAIN = 0x0000;
const TAG = 'fleet';

/// Receiver end of system sharing.
///
/// Declaring sendData skill only on EntryAbility is not enough — on real hardware
/// utd gets registered in bm dump, but Fleet still doesn't appear in the share
/// panel. System share panels require receivers to provide a share-type UIExtension;
/// ShareExtensionAbility exists for this in the SDK (@ohos.app.ability.ShareExtensionAbility,
/// inherits UIExtensionAbility).
///
/// We just forward here: hand the want to the small page inside the panel, which
/// writes content to preferences shared with the main app, then launches the main
/// app. No full interaction in the share panel — users mean "throw this at Fleet",
/// not "operate Fleet in someone else's panel".
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
