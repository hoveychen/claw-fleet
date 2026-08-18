import { t } from '../common/I18n';
import { ColorTokens, themeTokens } from '../common/Theme';
import { RelayClient } from '../common/Relay';
import { BrowseDirResponse, BrowseEntry } from '../models/Types';
import { Icon } from '../common/Icon';

/// Directory picker overlay — the native port of mobile-web/src/views/DirPicker.tsx.
/// Lets a phone pick a directory that lives on the *desktop* host, one level at a
/// time via the relay's `browse_dir` (which also decides "can we go up" — a null
/// parent marks the browsable boundary, so we never join paths ourselves).
///
/// ArkTS shape notes: @Builder bodies hold only UI syntax (derived values live in
/// regular methods); relay params are declared interfaces (BrowseParams), never
/// Record literals.
interface BrowseParams {
  path: string;
}

@Component
export struct DirPicker {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  @StorageProp('lang') lang: string = 'zh';

  client: RelayClient | null = null;
  /// Starting directory; empty means start at the desktop host's home.
  initialPath: string = '';
  onPick: (path: string) => void = () => {};
  onClose: () => void = () => {};

  @State private data: BrowseDirResponse | null = null;
  @State private error: string = '';
  @State private loading: boolean = true;

  private tk(): ColorTokens {
    return themeTokens(this.resolvedTheme);
  }

  aboutToAppear(): void {
    this.load(this.initialPath, true);
  }

  private load(path?: string, fallbackToHome?: boolean): void {
    const client = this.client;
    if (!client) {
      return;
    }
    this.loading = true;
    this.error = '';
    const params: BrowseParams | undefined =
      path !== undefined && path.length > 0 ? { path } : undefined;
    client.request<BrowseDirResponse>('browse_dir', params)
      .then((resp: BrowseDirResponse) => {
        this.data = resp;
      })
      .catch((e: Object) => {
        // Keep the previous screen so the user can still back out.
        this.error = e instanceof Error ? (e as Error).message : t('读取目录失败');
        // But if the *start* dir can't be opened there is no previous screen —
        // the hand-typed path may point at a long-deleted directory. Fall back to
        // home so the picker is always navigable; the error still explains why.
        if (fallbackToHome === true && params !== undefined) {
          client.request<BrowseDirResponse>('browse_dir', undefined)
            .then((home: BrowseDirResponse) => {
              this.data = home;
            })
            .catch((_e: Object) => {})
            .finally(() => {
              this.loading = false;
            });
          return;
        }
      })
      .finally(() => {
        this.loading = false;
      });
  }

  private crumb(): string {
    if (this.data !== null) {
      return this.data.path;
    }
    return this.initialPath.length > 0 ? this.initialPath : '…';
  }

  build() {
    Column() {
      // Backdrop tap closes.
      Column()
        .layoutWeight(1)
        .width('100%')
        .onClick(() => this.onClose())

      Column({ space: 10 }) {
        // Head
        Row() {
          Text(t('选择工作目录'))
            .fontSize(16)
            .fontWeight(FontWeight.Bold)
            .fontColor(this.tk().text)
            .layoutWeight(1)
          Icon({ src: $r('app.media.ic_x'), iconSize: 18, color: this.tk().textDim })
            .onClick(() => this.onClose())
        }
        .width('100%')

        // Breadcrumb (current path)
        Text(this.crumb())
          .fontSize(12)
          .fontColor(this.tk().textSecondary)
          .maxLines(1)
          .textOverflow({ overflow: TextOverflow.Ellipsis })
          .width('100%')

        if (this.error.length > 0) {
          Text(this.error)
            .fontSize(12)
            .fontColor(this.tk().error)
            .width('100%')
        }

        // List of subdirectories
        Scroll() {
          Column() {
            if (this.data !== null && this.data.parent !== null) {
              this.rowItem($r('app.media.ic_chevron_up'), t('上一级'), () => {
                const parent = this.data?.parent;
                if (parent !== null && parent !== undefined) {
                  this.load(parent);
                }
              })
            }
            if (this.data !== null) {
              ForEach(this.data.entries ?? [], (e: BrowseEntry) => {
                this.rowItem(e.isGitRepo ? $r('app.media.ic_folder_git_2') : $r('app.media.ic_folder'), e.name, () => this.load(e.path))
              }, (e: BrowseEntry) => e.path)
            }
            if (this.data !== null && !this.loading && (this.data.entries ?? []).length === 0) {
              Text(t('这里没有子目录'))
                .fontSize(13)
                .fontColor(this.tk().textDim)
                .padding(12)
            }
            if (this.data !== null && this.data.truncated) {
              Text(t('子目录过多，仅显示前 500 个'))
                .fontSize(12)
                .fontColor(this.tk().textDim)
                .padding(12)
            }
            if (this.loading) {
              Text(t('读取中…'))
                .fontSize(13)
                .fontColor(this.tk().textDim)
                .padding(12)
            }
          }
          .width('100%')
        }
        .layoutWeight(1)
        .width('100%')
        .scrollBar(BarState.Off)

        Button(t('用这个目录'))
          .width('100%')
          .fontSize(14)
          .fontWeight(600)
          .borderRadius(10)
          .fontColor('#ffffff')
          .backgroundColor(this.tk().accent)
          .enabled(this.data !== null)
          .onClick(() => {
            const d = this.data;
            if (d !== null) {
              this.onPick(d.path);
            }
          })
      }
      .width('100%')
      .height('72%')
      .padding(16)
      .backgroundColor(this.tk().bgCard)
      .borderRadius({ topLeft: 16, topRight: 16 })
      .border({ width: { top: 1 }, color: this.tk().border })
    }
    .width('100%')
    .height('100%')
    .backgroundColor('#99000000')
  }

  @Builder
  rowItem(icon: Resource, name: string, onTap: () => void) {
    Row({ space: 10 }) {
      Icon({ src: icon, iconSize: 16, color: this.tk().textSecondary })
      Text(name)
        .fontSize(14)
        .fontColor(this.tk().text)
        .layoutWeight(1)
        .maxLines(1)
        .textOverflow({ overflow: TextOverflow.Ellipsis })
    }
    .width('100%')
    .padding({ top: 11, bottom: 11, left: 4, right: 4 })
    .onClick(onTap)
  }
}
