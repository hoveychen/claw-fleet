import { t, dateLocale } from '../common/I18n';
import { ColorTokens, themeTokens } from '../common/Theme';
import { pushBackLayer, dropBackLayer } from '../common/BackStack';
import { RelayClient } from '../common/Relay';
import { RepoSummary, RepoDetail, WorktreeHealth, CommitInfo, GitOpResult, DirtyFile } from '../models/Types';
import { Icon } from '../common/Icon';
import { CAPSULE_INSET_RIGHT, HEADER_HEIGHT } from '../common/SafeArea';

/// 单个仓库详情 — the native port of mobile-web/src/views/RepoDetailView.tsx. The
/// main branch's push status, each worktree's loose-ends detail, recent commits,
/// and push/pull buttons (with a confirm, showing git output and refreshing after).
/// Data over the relay's `repo_detail` / `repo_push` / `repo_pull`.

interface RootParams {
  root: string;
}

/// Unix seconds → localized short date-time.
function fmtTime(sec: number | null | undefined): string {
  if (sec === null || sec === undefined || sec === 0) {
    return '';
  }
  return new Date(sec * 1000).toLocaleDateString(dateLocale(), {
    year: '2-digit',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  });
}

@Component
export struct RepoDetailView {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  @StorageProp('lang') lang: string = 'zh';
  @StorageProp('safeTop') safeTop: number = 0;

  repo: RepoSummary = {} as RepoSummary;
  client: RelayClient | null = null;
  onBack: () => void = () => {};

  @State private detail: RepoDetail | null = null;
  @State private error: string = '';
  @State private busy: string = ''; // '' | 'push' | 'pull'
  @State private opResult: GitOpResult | null = null;
  // Whether the main checkout's "脏 N" badge is expanded into its file list.
  @State private showMainFiles: boolean = false;
  // Worktree paths whose "脏 N" badge is expanded (mobile has no file tree, so
  // the list is read-only — display-only, no reveal).
  @State private expandedWts: string[] = [];
  private backLayerId: number = -1;

  private tk(): ColorTokens {
    return themeTokens(this.resolvedTheme);
  }

  private isWtExpanded(path: string): boolean {
    return this.expandedWts.includes(path);
  }

  private toggleWt(path: string): void {
    this.expandedWts = this.expandedWts.includes(path)
      ? this.expandedWts.filter((x: string) => x !== path)
      : [...this.expandedWts, path];
  }

  /// One-char git status code → a token colour (matches the desktop 文件 tab).
  private statusColor(status: string): ResourceColor {
    const tk = this.tk();
    if (status === 'A') {
      return tk.success;
    }
    if (status === 'D' || status === 'U') {
      return tk.error;
    }
    if (status === 'M' || status === 'T') {
      return tk.warning;
    }
    if (status === 'R') {
      return tk.accent;
    }
    return tk.textDim;
  }

  aboutToAppear(): void {
    this.backLayerId = pushBackLayer(() => this.onBack());
    this.refresh();
  }

  aboutToDisappear(): void {
    dropBackLayer(this.backLayerId);
  }

  private refresh(): void {
    const client = this.client;
    if (!client) {
      return;
    }
    this.error = '';
    const params: RootParams = { root: this.repo.root };
    client.request<RepoDetail>('repo_detail', params)
      .then((d: RepoDetail) => {
        // ArkTS's typed JSON.parse can leave nested array fields undefined when
        // they arrive through the generic `data: Object` reply path — the same
        // reason MainShell guards pending_snapshot's arrays with `?? []`. Backfill
        // here so build()'s `.length` / ForEach never dereference undefined and
        // abort the app (device stack: RepoDetailView.ets worktrees.length →
        // TypeError: Cannot read property length of undefined).
        d.worktrees = d.worktrees ?? [];
        d.commits = d.commits ?? [];
        d.dirtyFiles = d.dirtyFiles ?? [];
        for (const w of d.worktrees) {
          w.dirtyFiles = w.dirtyFiles ?? [];
        }
        this.detail = d;
      })
      .catch((e: Object) => {
        this.error = e instanceof Error ? (e as Error).message : `${e}`;
      });
  }

  private confirm(message: string): Promise<boolean> {
    return new Promise<boolean>((resolve: (v: boolean) => void) => {
      AlertDialog.show({
        message,
        autoCancel: true,
        alignment: DialogAlignment.Center,
        primaryButton: { value: t('确定'), action: () => resolve(true) },
        secondaryButton: { value: t('取消'), action: () => resolve(false) },
        cancel: () => resolve(false),
      });
    });
  }

  private async runOp(op: string): Promise<void> {
    const client = this.client;
    if (!client || this.busy.length > 0) {
      return;
    }
    const ok = await this.confirm(op === 'push' ? t('确认 push 到远端？') : t('确认 pull（--ff-only）？'));
    if (!ok) {
      return;
    }
    this.busy = op;
    this.opResult = null;
    const params: RootParams = { root: this.repo.root };
    try {
      const res = await client.request<GitOpResult>(op === 'push' ? 'repo_push' : 'repo_pull', params);
      this.opResult = res;
      this.refresh();
    } catch (e) {
      const msg = e instanceof Error ? (e as Error).message : `${e}`;
      this.opResult = { ok: false, output: msg };
    } finally {
      this.busy = '';
    }
  }

  @Builder
  header() {
    Row() {
      Row({ space: 2 }) {
        Icon({ src: $r('app.media.ic_chevron_left'), iconSize: 22, color: this.tk().accent })
        Text(t('仓库')).fontSize(15).fontColor(this.tk().accent)
      }
      .onClick(() => this.onBack())
      Column() {
        Text(this.repo.label)
          .fontSize(15)
          .fontWeight(FontWeight.Bold)
          .fontColor(this.tk().text)
          .maxLines(1)
          .textOverflow({ overflow: TextOverflow.Ellipsis })
        Text(this.repo.root)
          .fontSize(11)
          .fontColor(this.tk().textDim)
          .maxLines(1)
          .textOverflow({ overflow: TextOverflow.Ellipsis })
      }
      .alignItems(HorizontalAlign.Start)
      .layoutWeight(1)
      .margin({ left: 8 })
    }
    .width('100%')
    .height(HEADER_HEIGHT)
    .padding({ left: 12, right: CAPSULE_INSET_RIGHT })
    .backgroundColor(this.tk().bgGlass)
    .border({ width: { bottom: 1 }, color: this.tk().border })
  }

  @Builder
  sectionLabel(label: string) {
    Text(label).fontSize(13).fontColor(this.tk().textDim).width('100%').margin({ top: 18, bottom: 8 })
  }

  @Builder
  chip(text: string, tone: string) {
    Text(text)
      .fontSize(11)
      .fontColor(tone === 'warn'
        ? this.tk().warning
        : (tone === 'ok'
          ? this.tk().success
          : (tone === 'muted' ? this.tk().textDim : this.tk().textSecondary)))
      .backgroundColor(this.tk().bgInput)
      .padding({ left: 7, right: 7, top: 2, bottom: 2 })
      .borderRadius(6)
      .margin({ left: 6 })
  }

  /// Clickable "脏 N" chip for the main checkout — toggles the file list.
  @Builder
  dirtyChipMain(count: number) {
    Row() {
      Text(t('脏 {0}', count)).fontSize(11).fontColor(this.tk().textSecondary)
      Icon({
        src: this.showMainFiles ? $r('app.media.ic_chevron_down') : $r('app.media.ic_chevron_right'),
        iconSize: 12,
        color: this.tk().textSecondary,
      })
    }
    .backgroundColor(this.tk().bgInput)
    .padding({ left: 7, right: 4, top: 2, bottom: 2 })
    .borderRadius(6)
    .margin({ left: 6 })
    .onClick(() => {
      this.showMainFiles = !this.showMainFiles;
    })
  }

  /// Clickable "脏 N" chip for one worktree — toggles that worktree's file list.
  @Builder
  dirtyChipWt(w: WorktreeHealth) {
    Row() {
      Text(t('脏 {0}', w.dirtyCount)).fontSize(11).fontColor(this.tk().textSecondary)
      Icon({
        src: this.isWtExpanded(w.path) ? $r('app.media.ic_chevron_down') : $r('app.media.ic_chevron_right'),
        iconSize: 12,
        color: this.tk().textSecondary,
      })
    }
    .backgroundColor(this.tk().bgInput)
    .padding({ left: 7, right: 4, top: 2, bottom: 2 })
    .borderRadius(6)
    .margin({ left: 6 })
    .onClick(() => this.toggleWt(w.path))
  }

  /// Read-only uncommitted-file list (path + one-char status colour).
  @Builder
  dirtyFileList(files: DirtyFile[]) {
    Column({ space: 2 }) {
      ForEach(files, (f: DirtyFile) => {
        Row({ space: 8 }) {
          Text(f.status)
            .fontSize(11)
            .fontFamily('monospace')
            .fontWeight(700)
            .fontColor(this.statusColor(f.status))
            .width(14)
            .textAlign(TextAlign.Center)
          Text(f.path)
            .fontSize(11)
            .fontFamily('monospace')
            .fontColor(this.tk().textSecondary)
            .maxLines(1)
            .textOverflow({ overflow: TextOverflow.Ellipsis })
            .layoutWeight(1)
        }
        .width('100%')
      }, (f: DirtyFile) => f.path)
    }
    .width('100%')
    .padding({ left: 6, right: 6, top: 6, bottom: 6 })
    .margin({ top: 6 })
    .borderRadius(8)
    .backgroundColor(this.tk().bgInput)
  }

  @Builder
  branchCard(d: RepoDetail) {
    Column() {
      Row() {
        Text(d.branch ?? t('(游离 HEAD)')).fontSize(14).fontColor(this.tk().text).layoutWeight(1)
        Text(d.upstream ?? t('无上游')).fontSize(13).fontColor(this.tk().textSecondary)
      }
      .width('100%')
      .padding({ top: 8, bottom: 8 })
      Divider().color(this.tk().border)
      Row() {
        Text(t('未推 / 落后')).fontSize(14).fontColor(this.tk().textSecondary).layoutWeight(1)
        Flex({ wrap: FlexWrap.Wrap, justifyContent: FlexAlign.End }) {
          this.chip(t('未推 {0}', d.unpushed ?? 0), (d.unpushed ?? 0) > 0 ? 'warn' : 'muted')
          this.chip(t('落后 {0}', d.behind ?? 0), (d.behind ?? 0) > 0 ? 'info' : 'muted')
          if (d.dirtyCount > 0) {
            this.dirtyChipMain(d.dirtyCount)
          }
          if ((d.unpushed ?? 0) === 0 && (d.behind ?? 0) === 0 && d.dirtyCount === 0) {
            this.chip(t('干净'), 'ok')
          }
        }
        .layoutWeight(1)
      }
      .width('100%')
      .padding({ top: 8, bottom: 8 })
    }
    .width('100%')
    .padding({ left: 14, right: 14 })
    .borderRadius(10)
    .backgroundColor(this.tk().bgCard)
    .border({ width: 1, color: this.tk().border })
  }

  @Builder
  opButtons() {
    Row({ space: 10 }) {
      Button(this.busy === 'pull' ? t('拉取中…') : t('Pull'))
        .layoutWeight(1)
        .fontSize(14)
        .fontWeight(600)
        .fontColor(this.tk().text)
        .backgroundColor(this.tk().bgCard)
        .border({ width: 1, color: this.tk().border })
        .borderRadius(9)
        .enabled(this.busy.length === 0 && this.client !== null)
        .onClick(() => this.runOp('pull'))
      Button(this.busy === 'push' ? t('推送中…') : t('Push'))
        .layoutWeight(1)
        .fontSize(14)
        .fontWeight(600)
        .fontColor('#ffffff')
        .backgroundColor(this.tk().accent)
        .borderRadius(9)
        .enabled(this.busy.length === 0 && this.client !== null)
        .onClick(() => this.runOp('push'))
    }
    .width('100%')
    .margin({ top: 10 })
  }

  @Builder
  worktreeRow(w: WorktreeHealth) {
    Row({ space: 8 }) {
      Circle({ width: 8, height: 8 })
        .fill(w.unmerged > 0 || w.dirtyCount > 0 ? this.tk().warning : this.tk().success)
      Column({ space: 3 }) {
        Text(w.branch ?? t('(游离 HEAD)'))
          .fontSize(13)
          .fontColor(this.tk().text)
          .maxLines(1)
          .textOverflow({ overflow: TextOverflow.Ellipsis })
          .width('100%')
        Flex({ wrap: FlexWrap.Wrap }) {
          if (w.unmerged > 0) {
            this.chip(t('未合并 {0}', w.unmerged), 'warn')
          }
          if (w.dirtyCount > 0) {
            this.dirtyChipWt(w)
          }
          if (w.unmerged === 0 && w.dirtyCount === 0) {
            this.chip(t('已合并'), 'ok')
          }
          if (fmtTime(w.lastCommitTime).length > 0) {
            Text(fmtTime(w.lastCommitTime)).fontSize(11).fontColor(this.tk().textDim).margin({ left: 4 })
          }
        }
        if (this.isWtExpanded(w.path) && w.dirtyCount > 0) {
          this.dirtyFileList(w.dirtyFiles)
        }
      }
      .alignItems(HorizontalAlign.Start)
      .layoutWeight(1)
    }
    .width('100%')
    .alignItems(VerticalAlign.Top)
    .padding({ top: 10, bottom: 10 })
  }

  @Builder
  commitRow(c: CommitInfo) {
    Row({ space: 8 }) {
      Text(c.hash).fontSize(12).fontColor(this.tk().accent).fontFamily('monospace')
      Column({ space: 2 }) {
        Text(c.summary)
          .fontSize(13)
          .fontColor(this.tk().text)
          .maxLines(2)
          .textOverflow({ overflow: TextOverflow.Ellipsis })
          .width('100%')
        Text(`${c.author} · ${fmtTime(c.time)}`).fontSize(11).fontColor(this.tk().textDim)
      }
      .alignItems(HorizontalAlign.Start)
      .layoutWeight(1)
    }
    .width('100%')
    .alignItems(VerticalAlign.Top)
    .padding({ top: 10, bottom: 10 })
  }

  build() {
    Column() {
      this.header()
      Scroll() {
        Column() {
          if (this.error.length > 0) {
            Text(t('仓库加载失败：{0}', this.error))
              .fontSize(13).fontColor(this.tk().error).width('100%').margin({ top: 16 })
          } else if (this.detail === null) {
            Text(t('加载中…')).fontSize(13).fontColor(this.tk().textDim).width('100%').margin({ top: 20 })
          } else {
            this.sectionLabel(t('当前分支'))
            this.branchCard(this.detail)
            if (this.showMainFiles && this.detail.dirtyCount > 0) {
              this.dirtyFileList(this.detail.dirtyFiles)
            }
            this.opButtons()
            if (this.opResult !== null) {
              Text(this.opResult.output.length > 0 ? this.opResult.output : (this.opResult.ok ? t('完成') : t('失败')))
                .fontSize(12)
                .fontFamily('monospace')
                .fontColor(this.opResult.ok ? this.tk().textSecondary : this.tk().error)
                .backgroundColor(this.tk().bgInput)
                .padding(10)
                .borderRadius(8)
                .width('100%')
                .margin({ top: 8 })
            }

            this.sectionLabel(t('worktree（{0}）', this.detail.worktrees.length))
            if (this.detail.worktrees.length === 0) {
              Text(t('没有 worktree。')).fontSize(13).fontColor(this.tk().textDim).width('100%')
            } else {
              Column() {
                ForEach(this.detail.worktrees, (w: WorktreeHealth) => {
                  this.worktreeRow(w)
                }, (w: WorktreeHealth) => w.path)
              }
              .width('100%')
              .padding({ left: 14, right: 14 })
              .borderRadius(10)
              .backgroundColor(this.tk().bgCard)
              .border({ width: 1, color: this.tk().border })
            }

            this.sectionLabel(t('最近提交'))
            if (this.detail.commits.length === 0) {
              Text(t('没有提交。')).fontSize(13).fontColor(this.tk().textDim).width('100%')
            } else {
              Column() {
                ForEach(this.detail.commits, (c: CommitInfo, idx: number) => {
                  this.commitRow(c)
                }, (c: CommitInfo, idx: number) => `${c.hash}-${idx}`)
              }
              .width('100%')
              .padding({ left: 14, right: 14 })
              .borderRadius(10)
              .backgroundColor(this.tk().bgCard)
              .border({ width: 1, color: this.tk().border })
            }
          }
        }
        .width('100%')
        .alignItems(HorizontalAlign.Start)
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
