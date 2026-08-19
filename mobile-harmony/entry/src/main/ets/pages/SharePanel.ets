import { common, Want } from '@kit.AbilityKit';
import { fileIo as fs } from '@kit.CoreFileKit';
import { hilog } from '@kit.PerformanceAnalysisKit';

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
      hilog.info(DOMAIN, TAG, 'share-diag action=%{public}s keys=%{public}s',
        want?.action ?? '(none)',
        params === undefined ? '(no params)' : JSON.stringify(Object.keys(params)));
      const texts: string[] = [];
      const files: string[] = [];

      // 文件走 `ability.params.stream`：真机日志里图片 uri 就直接躺在这个数组
      // 里（file://media/Photo/...）。此前按二手资料写的
      // `ability.want.params.udKey` + UDMF queryData 在真机上根本不存在那个
      // key，所以永远提取为空 —— 这是「分享列表里有 Fleet，点进去却说没有
      // 内容」的真正原因。
      const stream = params?.['ability.params.stream'];
      if (Array.isArray(stream)) {
        // 必须**在这里**把字节拷走。分享给的是媒体库 uri，读权限是临时授给
        // 本次分享的；面板一关、或换到主应用进程，再去读同一个 uri 就未必有
        // 权限了。拷进应用沙箱（同 bundle 的进程共享文件目录）之后，主应用
        // 才能稳稳读到。
        const dir: string = ctx.filesDir + '/shared';
        if (!fs.accessSync(dir)) {
          fs.mkdirSync(dir);
        }
        for (const entry of stream as Object[]) {
          const uri: string = String(entry);
          if (uri.length === 0) { continue; }
          const slash: number = uri.lastIndexOf('/');
          const name: string = slash >= 0 ? uri.substring(slash + 1) : uri;
          try {
            const srcFile = fs.openSync(uri, fs.OpenMode.READ_ONLY);
            const dstPath: string = dir + '/' + name;
            try {
              fs.copyFileSync(srcFile.fd, dstPath);
            } finally {
              fs.closeSync(srcFile);
            }
            const stat = fs.statSync(dstPath);
            hilog.info(DOMAIN, TAG, 'share-diag copied %{public}s bytes=%{public}d', name, stat.size);
            files.push(name);
          } catch (err) {
            const e2 = err as Error;
            hilog.warn(DOMAIN, TAG, 'share-diag copy failed %{public}s: %{public}s', name, e2.message);
          }
        }
      }

      // 纯文本分享不带 stream，文本在这几个键之一。逐个试而不是赌某一个。
      const textKeys: string[] = [
        'ability.want.params.text',
        'ability.params.text',
        'ohos.extra.param.key.contentTitle',
        'title',
        'text',
      ];
      for (const k of textKeys) {
        const v = params?.[k];
        if (typeof v === 'string' && v.length > 0) {
          texts.push(v);
        }
      }
      if (texts.length === 0 && want?.uri !== undefined && want.uri.length > 0) {
        texts.push(want.uri);
      }

      const text = texts.join('\n\n');
      if (text.length === 0 && files.length === 0) {
        hilog.warn(DOMAIN, TAG, 'share-diag nothing extracted');
        this.status = '没有可接收的内容';
        return;
      }
      hilog.info(DOMAIN, TAG, 'share-diag extracted text=%{public}d files=%{public}d',
        text.length, files.length);
      // 内容直接挂在 startAbility 的 parameters 上，不走跨进程存储。
      //
      // 先前用 preferences 交接，真机日志里主应用 drain 到的却是 (empty) ——
      // UIExtension 与 UIAbility 是两个进程，preferences 未必落在同一沙箱路径，
      // 而且还要赌「extension 写完」早于「主应用读取」。want 参数没有这两个
      // 问题：内容跟着拉起动作一起过去，天然有序。
      await ctx.startAbility({
        bundleName: 'com.atomicservice.6917610791622358675',
        abilityName: 'EntryAbility',
        parameters: {
          'fleet.share.text': text,
          'fleet.share.files': files.join('\n'),
        },
      });
      hilog.info(DOMAIN, TAG, 'share-diag startAbility done');
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
