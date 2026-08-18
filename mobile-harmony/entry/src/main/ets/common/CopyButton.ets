/// Copy-to-clipboard button — the native mirror of mobile-web's <CopyButton>.
/// Same idle → done/failed state machine (auto-resets after ~1.4s), but writes
/// via the HarmonyOS system pasteboard (`@kit.BasicServicesKit`) instead of the
/// browser clipboard API, and renders lucide icons through the shared Icon
/// component. The pasteboard write is awaited so a blocked write flips to the
/// "failed" glyph rather than falsely flashing "copied".

import { pasteboard } from '@kit.BasicServicesKit';
import { Icon } from './Icon';
import { ColorTokens, themeTokens } from './Theme';

@Component
export struct CopyButton {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  @Require @Prop text: string;
  @Prop iconSize: number = 14;
  @State private state: string = 'idle'; // 'idle' | 'done' | 'failed'
  private resetTimer: number = -1;

  aboutToDisappear(): void {
    if (this.resetTimer !== -1) {
      clearTimeout(this.resetTimer);
    }
  }

  private tk(): ColorTokens {
    return themeTokens(this.resolvedTheme);
  }

  private iconRes(): Resource {
    if (this.state === 'done') {
      return $r('app.media.ic_check');
    }
    if (this.state === 'failed') {
      return $r('app.media.ic_x');
    }
    return $r('app.media.ic_copy');
  }

  private iconColor(): string {
    if (this.state === 'done') {
      return this.tk().success;
    }
    if (this.state === 'failed') {
      return this.tk().error;
    }
    return this.tk().textDim;
  }

  private scheduleReset(): void {
    if (this.resetTimer !== -1) {
      clearTimeout(this.resetTimer);
    }
    this.resetTimer = setTimeout(() => {
      this.state = 'idle';
      this.resetTimer = -1;
    }, 1400);
  }

  private doCopy(): void {
    const data = pasteboard.createData(pasteboard.MIMETYPE_TEXT_PLAIN, this.text);
    pasteboard.getSystemPasteboard().setData(data)
      .then(() => {
        this.state = 'done';
        this.scheduleReset();
      })
      .catch(() => {
        this.state = 'failed';
        this.scheduleReset();
      });
  }

  build() {
    Icon({ src: this.iconRes(), iconSize: this.iconSize, color: this.iconColor() })
      .padding(4)
      .onClick(() => this.doCopy())
  }
}
