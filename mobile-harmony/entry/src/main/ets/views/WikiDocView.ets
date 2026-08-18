import { buffer } from '@kit.ArkTS';
import { promptAction } from '@kit.ArkUI';
import { common } from '@kit.AbilityKit';
import { t, dateLocale } from '../common/I18n';
import { exportWikiDoc } from '../common/FileTransfer';
import { pushBackLayer, dropBackLayer } from '../common/BackStack';
import { ColorTokens, themeTokens } from '../common/Theme';
import { RelayClient } from '../common/Relay';
import { MarkdownView } from '../common/Markdown';
import { WikiDoc, WikiVersion, WikiFilePayload } from '../models/Types';
import { Icon } from '../common/Icon';
import { CAPSULE_INSET_RIGHT, HEADER_HEIGHT } from '../common/SafeArea';

/// 全屏 wiki 阅读页 — the native port of mobile-web/src/views/WikiDocView.tsx.
/// Markdown docs render inline via the native MarkdownView. HTML / htmlDir docs
/// need a full ArkWeb browser surface, which the atomic-service (元服务) sandbox
/// restricts (see Markdown.ets) — they show a "view on desktop" notice instead.
/// A version selector switches between published versions.
///
/// `[[slug]]` tap navigation (feature-gaps P3): a doc is a chain, not a single
/// page — tapping an in-doc `[[slug]]` link resolves the slug via `wiki_list`,
/// pushes that doc onto an internal stack, and 返回 pops back one doc at a time
/// (mirrors mobile-web's onOpenDoc chain). The version selector resets to the
/// newly-current doc's latest.
///
/// Export (feature-gaps P5): the ⤴ header button fetches the current version via
/// wiki_export and saves it through DocumentViewPicker.save (@atomicservice). The
/// 元服务 SDK has no outbound share sheet (ShareKit absent), so this is save-to-
/// location, not a share sheet — the "分享" half degrades to the save dialog.
///
/// Deferred vs. web (P8 device validation): full-fidelity HTML rendering (needs
/// ArkWeb availability verified in 元服务 + the data-URI asset-inlining port).
///
/// ArkTS shape notes: relay params are declared interfaces; base64 → utf8 decode
/// goes through `buffer`.

interface FileParams {
  slug: string;
  version: string;
  relpath: string;
}

/// `[[slug]]` / `[[slug|label]]` → a markdown link `[label](wiki:slug)`. The
/// `wiki:` scheme is what MarkdownView keys on to render the run as a tappable
/// in-app link (firing onWikiLink with the raw slug). Slugs are constrained to
/// `[a-z0-9-/]` per the publish contract, so the raw slug never contains a `)`
/// that would break MarkdownView's `[text](url)` scan — no escaping needed.
function expandWikiMentions(md: string): string {
  return md.replace(/\[\[([^\]|]+?)(?:\|([^\]]+?))?\]\]/g, (_m: string, slug: string, label: string) => {
    const s = (slug ?? '').trim();
    const text = (label !== undefined && label.length > 0 ? label : s).trim();
    return `[${text}](wiki:${s})`;
  });
}

function fmtVersion(ms: number): string {
  return new Date(ms).toLocaleString(dateLocale(), {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  });
}

@Component
export struct WikiDocView {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  @StorageProp('lang') lang: string = 'zh';
  @StorageProp('safeTop') safeTop: number = 0;

  doc: WikiDoc = {} as WikiDoc;
  client: RelayClient | null = null;
  onBack: () => void = () => {};

  // Navigation chain: the entry doc plus every `[[slug]]` jump. `current()` is
  // the last element; 返回 pops until back at the entry, then calls onBack.
  @State private stack: WikiDoc[] = [];
  @State private version: string = '';
  @State private markdown: string | null = null;
  @State private error: string = '';
  @State private exporting: boolean = false;
  private backLayerId: number = -1;

  private tk(): ColorTokens {
    return themeTokens(this.resolvedTheme);
  }

  private ctx(): common.UIAbilityContext {
    return this.getUIContext().getHostContext() as common.UIAbilityContext;
  }

  /// Export the current doc version to a user-chosen location. Cancelling the
  /// save dialog is silent; a real failure toasts.
  private handleExport(): void {
    const client = this.client;
    if (!client || this.exporting) {
      return;
    }
    this.exporting = true;
    exportWikiDoc(client, this.current().slug, this.version, this.ctx())
      .then((saved: boolean) => {
        this.exporting = false;
        if (saved) {
          promptAction.showToast({ message: t('已导出到所选位置'), duration: 2000 });
        }
      })
      .catch((e: Object) => {
        this.exporting = false;
        const msg = e instanceof Error ? (e as Error).message : `${e}`;
        promptAction.showToast({ message: t('导出失败：{0}', msg), duration: 2500 });
      });
  }

  /// The doc currently on screen (top of the navigation stack). Falls back to
  /// the entry prop before aboutToAppear seeds the stack.
  private current(): WikiDoc {
    return this.stack.length > 0 ? this.stack[this.stack.length - 1] : this.doc;
  }

  aboutToAppear(): void {
    // One layer for the whole reader; its back pops the internal `[[slug]]` doc
    // chain one hop at a time (goBack), closing the overlay only at the entry doc.
    this.backLayerId = pushBackLayer(() => this.goBack());
    this.stack = [this.doc];
    this.showDoc(this.doc);
  }

  aboutToDisappear(): void {
    dropBackLayer(this.backLayerId);
  }

  /// Reset version + (re)load content for whatever doc just became current.
  private showDoc(d: WikiDoc): void {
    this.version = d.currentVersion;
    this.markdown = null;
    this.error = '';
    if (d.kind === 'markdown') {
      this.loadMarkdown();
    }
  }

  /// Resolve a `[[slug]]` tap to a doc (fresh `wiki_list`) and push it. A missing
  /// slug toasts; an offline list is silently ignored (matches mobile-web).
  private openSlug(slug: string): void {
    const client = this.client;
    if (!client) {
      return;
    }
    client.request<WikiDoc[]>('wiki_list')
      .then((list: WikiDoc[]) => {
        let target: WikiDoc | null = null;
        for (const d of list) {
          if (d.slug === slug) {
            target = d;
            break;
          }
        }
        if (target !== null) {
          this.stack = this.stack.concat([target]);
          this.showDoc(target);
        } else {
          promptAction.showToast({ message: t('知识库里没有找到「{0}」', slug), duration: 2500 });
        }
      })
      .catch((_e: Object) => {
        // offline — ignore
      });
  }

  /// 返回: pop one doc off the chain, or leave the reader when at the entry.
  private goBack(): void {
    if (this.stack.length > 1) {
      this.stack = this.stack.slice(0, this.stack.length - 1);
      this.showDoc(this.current());
    } else {
      this.onBack();
    }
  }

  private loadMarkdown(): void {
    const client = this.client;
    if (!client) {
      return;
    }
    this.markdown = null;
    this.error = '';
    const cur = this.current();
    const params: FileParams = { slug: cur.slug, version: this.version, relpath: cur.entry };
    client.request<WikiFilePayload>('wiki_file', params)
      .then((f: WikiFilePayload) => {
        this.markdown = buffer.from(f.base64, 'base64').toString('utf-8');
      })
      .catch((e: Object) => {
        this.error = e instanceof Error ? (e as Error).message : `${e}`;
      });
  }

  @Builder
  header() {
    Row({ space: 8 }) {
      Row({ space: 2 }) {
        Icon({ src: $r('app.media.ic_chevron_left'), iconSize: 18, color: this.tk().accent })
        Text(t('返回')).fontSize(15).fontColor(this.tk().accent)
      }
      .onClick(() => this.goBack())
      Column() {
        Text(this.current().title.length > 0 ? this.current().title : this.current().slug)
          .fontSize(15)
          .fontWeight(FontWeight.Bold)
          .fontColor(this.tk().text)
          .maxLines(1)
          .textOverflow({ overflow: TextOverflow.Ellipsis })
        Text(`${this.current().slug} · ${this.current().workspaceName}`)
          .fontSize(11)
          .fontColor(this.tk().textDim)
          .maxLines(1)
          .textOverflow({ overflow: TextOverflow.Ellipsis })
      }
      .alignItems(HorizontalAlign.Start)
      .layoutWeight(1)
      if (this.current().versions !== undefined && this.current().versions.length > 1) {
        Select(this.versionOptions())
          .selected(this.versionIdx())
          .value(this.versionLabel())
          .font({ size: 12 })
          .fontColor(this.tk().text)
          .backgroundColor(this.tk().bgInput)
          .borderRadius(6)
          .onSelect((i: number) => {
            this.version = this.current().versions[i].id;
            if (this.current().kind === 'markdown') {
              this.loadMarkdown();
            }
          })
      }
      // 导出 / 分享: web uses navigator.share (⤴). The 元服务 SDK has no outbound
      // share sheet, so this saves via DocumentViewPicker — same "⤴" glyph as
      // the web's .exportButton (18px, accent; dim while disabled).
      Text(this.exporting ? '…' : '⤴')
        .fontSize(18)
        .fontColor(this.exporting ? this.tk().textDim : this.tk().accent)
        .padding({ left: 6, right: 6, top: 4, bottom: 4 })
        .onClick(() => this.handleExport())
    }
    .width('100%')
    .height(HEADER_HEIGHT)
    .padding({ left: 12, right: CAPSULE_INSET_RIGHT })
    .backgroundColor(this.tk().bgGlass)
    .border({ width: { bottom: 1 }, color: this.tk().border })
  }

  private versionOptions(): SelectOption[] {
    const vers = this.current().versions ?? [];
    return vers.map((v: WikiVersion, i: number) => {
      const o: SelectOption = { value: i === 0 ? t('最新') : fmtVersion(v.publishedMs) };
      return o;
    });
  }

  private versionIdx(): number {
    const vers = this.current().versions ?? [];
    for (let i = 0; i < vers.length; i++) {
      if (vers[i].id === this.version) {
        return i;
      }
    }
    return 0;
  }

  private versionLabel(): string {
    const vers = this.current().versions ?? [];
    const idx = this.versionIdx();
    if (vers.length === 0) {
      return '';
    }
    return idx === 0 ? t('最新') : fmtVersion(vers[idx].publishedMs);
  }

  build() {
    Column() {
      this.header()
      Scroll() {
        Column() {
          if (this.current().kind === 'markdown') {
            if (this.error.length > 0) {
              Text(t('加载失败：{0}', this.error)).fontSize(13).fontColor(this.tk().error).width('100%')
            } else if (this.markdown === null) {
              Text(t('加载中…')).fontSize(13).fontColor(this.tk().textDim).width('100%').margin({ top: 20 })
            } else {
              MarkdownView({
                content: expandWikiMentions(this.markdown),
                mdFontSize: 15,
                onWikiLink: (slug: string) => this.openSlug(slug),
              })
            }
          } else {
            // HTML / htmlDir: full-fidelity render needs an ArkWeb surface the
            // 元服务 sandbox restricts. Point the user to the desktop.
            Column({ space: 10 }) {
              Icon({ src: $r('app.media.ic_monitor'), iconSize: 40, color: this.tk().textSecondary })
              Text(t('这是一个 HTML 页面'))
                .fontSize(16)
                .fontWeight(FontWeight.Medium)
                .fontColor(this.tk().text)
              Text(t('元服务暂不支持内联渲染 HTML 文档，请在桌面端 Fleet 的知识库中查看。'))
                .fontSize(13)
                .fontColor(this.tk().textSecondary)
                .textAlign(TextAlign.Center)
                .lineHeight(20)
            }
            .width('100%')
            .padding({ top: 60, left: 32, right: 32 })
            .justifyContent(FlexAlign.Center)
          }
        }
        .width('100%')
        .alignItems(HorizontalAlign.Start)
        .padding({ left: 16, right: 16, top: 12, bottom: 32 })
      }
      .layoutWeight(1)
      .width('100%')
      .align(Alignment.Top)
      .scrollBar(BarState.Off)
    }
    .width('100%')
    .height('100%')
    .backgroundColor(this.tk().bg)
  }
}
