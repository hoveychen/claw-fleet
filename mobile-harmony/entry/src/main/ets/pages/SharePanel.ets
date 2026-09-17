import { common, Want } from '@kit.AbilityKit';
import { fileIo as fs } from '@kit.CoreFileKit';
import { hilog } from '@kit.PerformanceAnalysisKit';

const DOMAIN = 0x0000;
const TAG = 'fleet';
const storage: LocalStorage = LocalStorage.getShared();

/// A small piece of UI in the sharing panel.
///
/// Deliberately minimal: when users tap Fleet, they mean "throw this content at
/// Fleet", not "operate Fleet again in someone else's sharing panel". So this just
/// writes content to shared preferences, launches the main app, and closes itself.
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

      // Files go in `ability.params.stream`: on real hardware, image URIs sit
      // directly in this array (file://media/Photo/...). Earlier we tried
      // `ability.want.params.udKey` + UDMF queryData (from secondhand docs),
      // but that key doesn't exist on real hardware, always empty — that's why
      // "Fleet shows in the share list but says there's no content".
      const stream = params?.['ability.params.stream'];
      if (Array.isArray(stream)) {
        // Must **copy the bytes here**. Sharing gives us media library URIs;
        // read permission is temporary for this share. Once the panel closes or
        // we switch to the main app process, the same URI might not have permission.
        // Copy into the app sandbox (processes in the same bundle share the file
        // directory), then the main app can reliably read it.
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

      // Pure text sharing has no stream; text is in one of these keys. Try them
      // all rather than betting on one.
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
      // Content goes directly on startAbility's parameters, not via cross-process storage.
      //
      // Earlier we used preferences for handoff, but real device logs showed the main
      // app always drained (empty) — UIExtension and UIAbility are two processes,
      // preferences might not land in the same sandbox path, and we'd have to bet
      // "extension write completes before main app reads". Want parameters don't have
      // those problems: they travel with the launch action, naturally ordered.
      await ctx.startAbility({
        // Read from our own context, not a literal — when bundle name changes, this follows.
        bundleName: ctx.extensionAbilityInfo.bundleName,
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

/// Session only uses terminateSelf, avoid dragging in the whole type for one method.
interface UIExtensionContentSessionLike {
  terminateSelf(): void;
}
