import { t } from '../common/I18n';
import { ColorTokens, themeTokens } from '../common/Theme';
import { pushBackLayer, dropBackLayer } from '../common/BackStack';
import { RelayClient } from '../common/Relay';
import { RepoSummary } from '../models/Types';
import { EmptyState } from './EmptyState';
import { Icon } from '../common/Icon';
import { CAPSULE_INSET_RIGHT, HEADER_HEIGHT } from '../common/SafeArea';

/// 仓库列表 — the native port of mobile-web/src/views/RepoView.tsx. Every git repo
/// reachable from a session workspace, each with a "loose-ends" health tag
/// (unpushed commits, un-merged/dirty worktrees). Tapping a repo opens
/// RepoDetailView. Data over the relay's `repo_list`.

@Component
export struct RepoView {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  @StorageProp('lang') lang: string = 'zh';
  @StorageProp('safeTop') safeTop: number = 0;

  client: RelayClient | null = null;
  onBack: () => void = () => {};
  onOpenRepo: (repo: RepoSummary) => void = () => {};

  @State private repos: RepoSummary[] | null = null;
  @State private error: string = '';
  private backLayerId: number = -1;

  private tk(): ColorTokens {
    return themeTokens(this.resolvedTheme);
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
    client.request<RepoSummary[]>('repo_list')
      .then((list: RepoSummary[]) => {
        this.repos = list;
      })
      .catch((e: Object) => {
        this.error = e instanceof Error ? (e as Error).message : `${e}`;
      });
  }

  @Builder
  header() {
    Row() {
      Row({ space: 2 }) {
        Icon({ src: $r('app.media.ic_chevron_left'), iconSize: 22, color: this.tk().accent })
        Text(t('更多')).fontSize(15).fontColor(this.tk().accent)
      }
      .onClick(() => this.onBack())
      Text(t('仓库'))
        .fontSize(16)
        .fontWeight(FontWeight.Bold)
        .fontColor(this.tk().text)
        .layoutWeight(1)
        .textAlign(TextAlign.Start)
        .margin({ left: 12 })
      Text('⟳').fontSize(18).fontColor(this.tk().text).onClick(() => this.refresh())
    }
    .width('100%')
    .height(HEADER_HEIGHT)
    .padding({ left: 12, right: CAPSULE_INSET_RIGHT })
    .backgroundColor(this.tk().bgGlass)
    .border({ width: { bottom: 1 }, color: this.tk().border })
  }

  @Builder
  badge(text: string, warn: boolean) {
    Text(text)
      .fontSize(11)
      .fontColor(warn ? this.tk().warning : this.tk().textSecondary)
      .backgroundColor(this.tk().bgInput)
      .padding({ left: 7, right: 7, top: 2, bottom: 2 })
      .borderRadius(6)
      .border({ width: 1, color: warn ? this.tk().warning : Color.Transparent })
  }

  @Builder
  badges(repo: RepoSummary) {
    Flex({ wrap: FlexWrap.Wrap }) {
      if ((repo.unpushed ?? 0) > 0) {
        this.badge(t('未推 {0}', repo.unpushed ?? 0), true)
      }
      if (repo.pendingWorktrees > 0) {
        this.badge(t('待合并 {0}', repo.pendingWorktrees), true)
      }
      if (repo.dirtyCount > 0) {
        this.badge(t('脏 {0}', repo.dirtyCount), false)
      }
      if ((repo.unpushed ?? 0) === 0 && repo.pendingWorktrees === 0 && repo.dirtyCount === 0) {
        if (repo.worktreeCount > 0) {
          this.badge(t('worktree {0}', repo.worktreeCount), false)
        }
        this.badge(t('干净'), false)
      }
    }
  }

  @Builder
  repoRow(repo: RepoSummary) {
    Row({ space: 10 }) {
      Icon({
        src: $r('app.media.ic_folder_git_2'),
        iconSize: 18,
        color: repo.needsAttention ? this.tk().warning : this.tk().textDim
      })
      Column({ space: 4 }) {
        Text(repo.label)
          .fontSize(15)
          .fontColor(this.tk().text)
          .maxLines(1)
          .textOverflow({ overflow: TextOverflow.Ellipsis })
          .width('100%')
        Row({ space: 6 }) {
          Text(repo.branch ?? t('(游离 HEAD)'))
            .fontSize(12)
            .fontColor(this.tk().textDim)
        }
        this.badges(repo)
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
    .onClick(() => this.onOpenRepo(repo))
  }

  build() {
    Column() {
      this.header()
      Scroll() {
        Column({ space: 8 }) {
          if (this.error.length > 0) {
            Text(t('仓库加载失败：{0}', this.error))
              .fontSize(13)
              .fontColor(this.tk().textSecondary)
              .width('100%')
          } else if (this.repos === null) {
            Text(t('加载中…')).fontSize(13).fontColor(this.tk().textDim).width('100%').margin({ top: 20 })
          } else if (this.repos.length === 0) {
            EmptyState({
              iconRes: $r('app.media.ic_folder_git_2'),
              title: t('没有发现 git 仓库'),
              subtitle: t('仓库来自桌面端各会话的工作目录，有会话在 git 仓库里工作时会出现在这里。'),
            })
          } else {
            ForEach(this.repos, (repo: RepoSummary) => {
              this.repoRow(repo)
            }, (repo: RepoSummary) => repo.root)
          }
        }
        .width('100%')
        .padding({ left: 16, right: 16, top: 12, bottom: 28 })
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
