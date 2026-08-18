import { common, Want } from '@kit.AbilityKit';
import { unifiedDataChannel } from '@kit.ArkData';
import { hilog } from '@kit.PerformanceAnalysisKit';
import { SharedInbox } from '../common/SharedInbox';

const DOMAIN = 0x0000;
const TAG = 'fleet';
const storage: LocalStorage = LocalStorage.getShared();

/// 分享面板里那一小块界面。
///
/// 刻意做得极简：用户点 Fleet 的意图就是「把这段内容丢给 Fleet」，不该在别人
/// 的分享面板里再操作一遍 Fleet。所以这里只负责把内容落进共享 preferences，
/// 然后拉起主应用、关掉自己。
@Entry(storage)
@Component
struct SharePanel {
  @State private status: string = '正在接收…';

  aboutToAppear(): void {
    this.receive();
  }

  private async receive(): Promise<void> {
    const want = storage.get<Want>('shareWant');
    const ctx = getContext(this) as common.UIExtensionContext;
    try {
      const params = want?.parameters as Record<string, Object> | undefined;
      const key = params?.['ability.want.params.udKey'];
      const texts: string[] = [];
      const files: string[] = [];
      if (typeof key === 'string' && key.length > 0) {
        // 分享内容不在 want 参数里,want 只带一把 key,真数据要拿它去 UDMF 查。
        const list = await unifiedDataChannel.queryData({
          intention: unifiedDataChannel.Intention.DATA_HUB,
          key: key,
        });
        for (const data of list) {
          for (const rec of data.getRecords()) {
            // 文本与文件要分开收：只认 PlainText 的话，分享一张图会走到
            // 「没有可接收的内容」，而图片恰恰是最常见的分享。
            const plain = rec as unifiedDataChannel.PlainText;
            if (plain.textContent !== undefined && plain.textContent.length > 0) {
              texts.push(plain.textContent);
              continue;
            }
            const img = rec as unifiedDataChannel.Image;
            const uri: string = img.imageUri !== undefined && img.imageUri.length > 0
              ? img.imageUri
              : (rec as unifiedDataChannel.File).uri ?? '';
            if (uri.length > 0) {
              const slash = uri.lastIndexOf('/');
              const name = slash >= 0 ? uri.substring(slash + 1) : uri;
              files.push(name + '|' + uri);
            }
          }
        }
      }
      const text = texts.join('\n\n');
      if (text.length === 0 && files.length === 0) {
        this.status = '没有可接收的内容';
        return;
      }
      await SharedInbox.put(ctx, text, files);
      // 拉起主应用去处理；分享面板本身随即关闭。
      await ctx.startAbility({
        bundleName: 'com.atomicservice.6917610791622358675',
        abilityName: 'EntryAbility',
      });
      storage.get<UIExtensionContentSessionLike>('shareSession')?.terminateSelf();
    } catch (e) {
      const err = e as Error;
      hilog.error(DOMAIN, TAG, 'share receive failed: %{public}s', err.message);
      this.status = '接收失败：' + err.message;
    }
  }

  build() {
    Column({ space: 12 }) {
      Text('Fleet').fontSize(20).fontWeight(FontWeight.Bold)
      Text(this.status).fontSize(13).opacity(0.7)
    }
    .width('100%')
    .padding(24)
    .justifyContent(FlexAlign.Center)
  }
}

/// session 只用到 terminateSelf，避免为一个方法把整个类型引进来。
interface UIExtensionContentSessionLike {
  terminateSelf(): void;
}
