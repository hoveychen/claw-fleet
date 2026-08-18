import { t, dateLocale } from '../common/I18n';
import { ColorTokens, themeTokens } from '../common/Theme';
import { RelayClient } from '../common/Relay';
import { WikiDoc, WikiSearchHit } from '../models/Types';
import { EmptyState } from './EmptyState';
import { Icon } from '../common/Icon';

/// 知识库 tab — the native port of mobile-web/src/views/WikiView.tsx. Lists every
/// `fleet wiki publish` doc; empty search groups by slug virtual-folder, ≥2 chars
/// runs relay full-text search (wiki_search). A workspace filter narrows the set.
/// Tapping a doc opens WikiDocView (owned by MainShell as an overlay).
///
/// ArkTS shape notes: relay params are declared interfaces; derived groupings live
/// in regular methods (a @Builder body holds only UI syntax).

const SEARCH_DEBOUNCE_MS = 300;

interface SearchParams {
  query: string;
}

interface DocGroup {
  folder: string;
  docs: WikiDoc[];
}

/// Doc's virtual folder: slug minus the last segment ("" when no slash).
function folderOf(slug: string): string {
  const i = slug.lastIndexOf('/');
  return i < 0 ? '' : slug.substring(0, i);
}

/// slug's last segment — display fallback when title is empty.
function leafOf(slug: string): string {
  const i = slug.lastIndexOf('/');
  return i < 0 ? slug : slug.substring(i + 1);
}

function kindBadge(kind: string): string {
  if (kind === 'markdown') {
    return 'MD';
  }
  if (kind === 'html') {
    return 'HTML';
  }
  return 'DIR';
}

/// Badge background per kind (mirrors WikiView.module.css: markdown=accent,
/// html=success, htmlDir=warning; unknown falls back to text-dim). White text.
function kindBadgeColor(kind: string, tk: ColorTokens): string {
  if (kind === 'markdown') {
    return tk.accent;
  }
  if (kind === 'html') {
    return tk.success;
  }
  if (kind === 'htmlDir') {
    return tk.warning;
  }
  return tk.textDim;
}

/// yy/mm/dd in the UI locale (native mirror of WikiView's fmtDate).
function fmtDate(ms: number): string {
  if (!ms) {
    return '';
  }
  return new Date(ms).toLocaleDateString(dateLocale(), {
    year: '2-digit',
    month: '2-digit',
    day: '2-digit',
  });
}

@Component
export struct WikiView {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  @StorageProp('lang') lang: string = 'zh';

  client: RelayClient | null = null;
  onOpenDoc: (doc: WikiDoc) => void = () => {};

  @State private docs: WikiDoc[] | null = null;
  @State private error: string = '';
  @State private query: string = '';
  @State private workspace: string = '';
  @State private searching: boolean = false;
  @State private matchSlugs: string[] = [];
  @State private snippetBySlug: Map<string, string> = new Map<string, string>();

  private searchTimer: number = -1;

  private tk(): ColorTokens {
    return themeTokens(this.resolvedTheme);
  }

  aboutToAppear(): void {
    this.refresh();
  }

  aboutToDisappear(): void {
    if (this.searchTimer !== -1) {
      clearTimeout(this.searchTimer);
      this.searchTimer = -1;
    }
  }

  private refresh(): void {
    const client = this.client;
    if (!client) {
      return;
    }
    this.error = '';
    client.request<WikiDoc[]>('wiki_list')
      .then((list: WikiDoc[]) => {
        list.sort((a: WikiDoc, b: WikiDoc) => b.updatedMs - a.updatedMs);
        this.docs = list;
      })
      .catch((e: Object) => {
        this.error = e instanceof Error ? (e as Error).message : `${e}`;
      });
  }

  private searchActive(): boolean {
    return this.query.trim().length >= 2;
  }

  private runSearch(): void {
    const client = this.client;
    const q = this.query.trim();
    if (!client || q.length < 2) {
      this.matchSlugs = [];
      this.snippetBySlug = new Map<string, string>();
      this.searching = false;
      return;
    }
    this.searching = true;
    if (this.searchTimer !== -1) {
      clearTimeout(this.searchTimer);
    }
    this.searchTimer = setTimeout(() => {
      const c = this.client;
      if (!c) {
        this.searching = false;
        return;
      }
      const params: SearchParams = { query: q };
      c.request<WikiSearchHit[]>('wiki_search', params)
        .then((hits: WikiSearchHit[]) => {
          const slugs: string[] = [];
          const snips = new Map<string, string>();
          for (const h of hits ?? []) {
            if (!slugs.includes(h.slug)) {
              slugs.push(h.slug);
            }
            if (!snips.has(h.slug)) {
              snips.set(h.slug, h.snippet);
            }
          }
          this.matchSlugs = slugs;
          this.snippetBySlug = snips;
        })
        .catch((_e: Object) => {
          this.matchSlugs = [];
          this.snippetBySlug = new Map<string, string>();
        })
        .finally(() => {
          this.searching = false;
        });
    }, SEARCH_DEBOUNCE_MS);
  }

  private matchesWorkspace(d: WikiDoc): boolean {
    return this.workspace.length === 0 || d.workspaceName === this.workspace;
  }

  /// Distinct workspace names across the loaded docs, sorted.
  private workspaces(): string[] {
    const names = new Set<string>();
    for (const d of this.docs ?? []) {
      names.add(d.workspaceName);
    }
    const out: string[] = [];
    names.forEach((n: string) => out.push(n));
    out.sort((a: string, b: string) => a.localeCompare(b));
    return out;
  }

  private wsFilterEntries(): string[] {
    const entries: string[] = [t('全部项目')];
    for (const w of this.workspaces()) {
      entries.push(w);
    }
    return entries;
  }

  private wsSelectOptions(): SelectOption[] {
    return this.wsFilterEntries().map((label: string) => {
      const o: SelectOption = { value: label };
      return o;
    });
  }

  private wsSelectedIdx(): number {
    if (this.workspace.length === 0) {
      return 0;
    }
    const ws = this.workspaces();
    for (let i = 0; i < ws.length; i++) {
      if (ws[i] === this.workspace) {
        return i + 1;
      }
    }
    return 0;
  }

  /// Browse-mode groups (empty/short query), workspace-filtered, folder-sorted.
  private groups(): DocGroup[] {
    const docs = this.docs;
    if (docs === null) {
      return [];
    }
    const byFolder = new Map<string, WikiDoc[]>();
    for (const d of docs) {
      if (!this.matchesWorkspace(d)) {
        continue;
      }
      const key = folderOf(d.slug);
      const arr = byFolder.get(key);
      if (arr !== undefined) {
        arr.push(d);
      } else {
        byFolder.set(key, [d]);
      }
    }
    const out: DocGroup[] = [];
    byFolder.forEach((docList: WikiDoc[], folder: string) => out.push({ folder, docs: docList }));
    out.sort((a: DocGroup, b: DocGroup) => a.folder.localeCompare(b.folder));
    return out;
  }

  /// Search-mode results: relay hit slugs resolved to docs, workspace-filtered.
  private results(): WikiDoc[] {
    const docs = this.docs;
    if (docs === null) {
      return [];
    }
    const bySlug = new Map<string, WikiDoc>();
    for (const d of docs) {
      bySlug.set(d.slug, d);
    }
    const out: WikiDoc[] = [];
    for (const slug of this.matchSlugs) {
      const d = bySlug.get(slug);
      if (d !== undefined && this.matchesWorkspace(d)) {
        out.push(d);
      }
    }
    return out;
  }

  @Builder
  docRow(doc: WikiDoc, snippet: string) {
    Row({ space: 10 }) {
      Text(kindBadge(doc.kind))
        .fontSize(10)
        .fontWeight(FontWeight.Bold)
        .fontColor('#ffffff')
        .backgroundColor(kindBadgeColor(doc.kind, this.tk()))
        .padding({ left: 6, right: 6, top: 3, bottom: 3 })
        .borderRadius(6)
      Column({ space: 3 }) {
        Text(doc.title.length > 0 ? doc.title : leafOf(doc.slug))
          .fontSize(14)
          .fontColor(this.tk().text)
          .maxLines(1)
          .textOverflow({ overflow: TextOverflow.Ellipsis })
          .width('100%')
        if (snippet.length > 0) {
          Text(snippet)
            .fontSize(12)
            .fontColor(this.tk().textSecondary)
            .maxLines(2)
            .textOverflow({ overflow: TextOverflow.Ellipsis })
            .width('100%')
        } else {
          Text(fmtDate(doc.updatedMs).length > 0 ? `${doc.workspaceName} · ${fmtDate(doc.updatedMs)}` : doc.workspaceName)
            .fontSize(11)
            .fontColor(this.tk().textDim)
            .maxLines(1)
            .textOverflow({ overflow: TextOverflow.Ellipsis })
            .width('100%')
        }
      }
      .alignItems(HorizontalAlign.Start)
      .layoutWeight(1)
      Icon({ src: $r('app.media.ic_chevron_right'), iconSize: 18, color: this.tk().textDim })
    }
    .width('100%')
    .padding(12)
    .backgroundColor(this.tk().bgCard)
    .borderRadius(10)
    .border({ width: 1, color: this.tk().border })
    .onClick(() => this.onOpenDoc(doc))
  }

  @Builder
  head() {
    Column({ space: 8 }) {
      Row() {
        Text(t('知识库')).fontSize(20).fontWeight(FontWeight.Bold).fontColor(this.tk().text).layoutWeight(1)
        if (this.docs !== null && this.docs.length > 0) {
          Text(`${this.docs.length}`).fontSize(13).fontColor(this.tk().textDim).margin({ right: 10 })
        }
        Text('⟳').fontSize(18).fontColor(this.tk().text).onClick(() => this.refresh())
      }
      .width('100%')

      Row({ space: 8 }) {
        Row({ space: 6 }) {
          Icon({ src: $r('app.media.ic_search'), iconSize: 14, color: this.tk().textDim })
          TextInput({ text: this.query, placeholder: t('搜索标题 / 正文…') })
            .layoutWeight(1)
            .fontSize(14)
            .fontColor(this.tk().text)
            .placeholderColor(this.tk().textDim)
            .backgroundColor(Color.Transparent)
            .padding({ top: 6, bottom: 6 })
            .onChange((v: string) => {
              this.query = v;
              this.runSearch();
            })
          if (this.searching) {
            LoadingProgress().width(16).height(16).color(this.tk().accent)
          }
        }
        .layoutWeight(1)
        .padding({ left: 10, right: 10 })
        .borderRadius(8)
        .backgroundColor(this.tk().bgInput)
        .border({ width: 1, color: this.tk().border })

        if (this.workspaces().length > 1) {
          Select(this.wsSelectOptions())
            .selected(this.wsSelectedIdx())
            .value(this.wsSelectedIdx() === 0 ? t('全部项目') : this.workspace)
            .font({ size: 13 })
            .fontColor(this.tk().text)
            .backgroundColor(this.tk().bgInput)
            .borderRadius(8)
            .constraintSize({ maxWidth: '40%' })
            .onSelect((i: number) => {
              this.workspace = i === 0 ? '' : this.workspaces()[i - 1];
            })
        }
      }
      .width('100%')
    }
    .width('100%')
    .padding({ left: 16, right: 16, top: 12, bottom: 8 })
  }

  build() {
    Column() {
      this.head()
      Scroll() {
        Column({ space: 8 }) {
          if (this.error.length > 0) {
            Text(t('知识库加载失败：{0}', this.error)).fontSize(13).fontColor(this.tk().error).width('100%')
          } else if (this.docs === null) {
            Text(t('加载中…')).fontSize(13).fontColor(this.tk().textDim).width('100%').margin({ top: 20 })
          } else if (this.docs.length === 0) {
            EmptyState({
              iconRes: $r('app.media.ic_book_open'),
              title: t('还没有归档的文档'),
              subtitle: t('桌面端 agent 用 fleet wiki publish 发布后，文档会出现在这里。'),
            })
          } else if (this.searchActive()) {
            if (this.searching) {
              Text(t('搜索中…')).fontSize(13).fontColor(this.tk().textDim).width('100%')
            } else if (this.results().length === 0) {
              EmptyState({ compact: true, iconRes: $r('app.media.ic_search_x'), title: t('没有匹配「{0}」的文档。', this.query) })
            } else {
              ForEach(this.results(), (doc: WikiDoc) => {
                this.docRow(doc, this.snippetBySlug.get(doc.slug) ?? '')
              }, (doc: WikiDoc) => doc.slug)
            }
          } else if (this.groups().length === 0) {
            EmptyState({ compact: true, iconRes: $r('app.media.ic_file_question_mark'), title: t('该项目下没有文档。') })
          } else {
            ForEach(this.groups(), (g: DocGroup) => {
              Text(g.folder.length > 0 ? g.folder : t('未归类'))
                .fontSize(12)
                .fontColor(this.tk().textDim)
                .width('100%')
                .margin({ top: 6, bottom: 2 })
              ForEach(g.docs, (doc: WikiDoc) => {
                this.docRow(doc, '')
              }, (doc: WikiDoc) => doc.slug)
            }, (g: DocGroup) => g.folder)
          }
        }
        .width('100%')
        .padding({ left: 16, right: 16, top: 4, bottom: 28 })
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
