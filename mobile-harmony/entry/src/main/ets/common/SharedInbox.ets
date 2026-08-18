import { common } from '@kit.AbilityKit';
import { preferences } from '@kit.ArkData';

/// 分享收件箱 —— 分享扩展与主应用之间的交接点。
///
/// 两者是不同的进程（UIExtension 独立于 UIAbility 运行），所以不能用
/// AppStorage 传递；preferences 落磁盘，主应用下次前台时读得到。
///
/// 只保留最后一条：用户连续分享两次而主应用一次都没来得及读的情况极少，
/// 为它做队列不划算；真丢了用户会再分享一次。
const STORE_NAME = 'fleet_share_inbox';
const KEY_TEXT = 'text';
const KEY_STAMP = 'stamp';

export class SharedInbox {
  static async put(context: common.Context, text: string): Promise<void> {
    const p = await preferences.getPreferences(context, { name: STORE_NAME });
    await p.put(KEY_TEXT, text);
    // 用递增计数而不是时间戳：同一段文本再分享一次也要能被认出来是新的一次。
    const n = await p.get(KEY_STAMP, 0) as number;
    await p.put(KEY_STAMP, n + 1);
    await p.flush();
  }

  /// 取走并清空。返回 null 表示没有待处理的分享。
  static async take(context: common.Context): Promise<string | null> {
    const p = await preferences.getPreferences(context, { name: STORE_NAME });
    const text = await p.get(KEY_TEXT, '') as string;
    if (text.length === 0) {
      return null;
    }
    await p.delete(KEY_TEXT);
    await p.flush();
    return text;
  }
}
