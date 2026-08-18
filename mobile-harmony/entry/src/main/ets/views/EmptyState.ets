import { ColorTokens, themeTokens } from '../common/Theme';
import { Icon } from '../common/Icon';

/// Centered icon + title + subtitle, the shared empty/placeholder state. Mirrors
/// mobile-web's <EmptyState>. Subscribes to the resolved theme for its colors.
///
///   - `spin`: swap the emoji icon for an animated LoadingProgress spinner (the
///     "connecting…" / "loading tasks…" states).
///   - `compact`: tighter vertical padding + smaller type, for an inline
///     "no matches" state that sits inside a list rather than filling the page.
@Component
export struct EmptyState {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  /// Legacy emoji fallback. Prefer `iconRes` (a lucide SVG) for new call sites;
  /// when both are set `iconRes` wins.
  @Prop icon: string = '';
  /// Lucide vector icon resource, e.g. `$r('app.media.ic_inbox')`.
  @Prop iconRes: Resource | undefined = undefined;
  @Prop title: string = '';
  @Prop subtitle: string = '';
  @Prop spin: boolean = false;
  @Prop compact: boolean = false;

  // Fade-in on mount, mirroring mobile-web's EmptyState fadeIn.
  @State private appeared: number = 0;

  private tk(): ColorTokens {
    return themeTokens(this.resolvedTheme);
  }

  aboutToAppear(): void {
    animateTo({ duration: 220, curve: Curve.EaseOut }, () => {
      this.appeared = 1;
    });
  }

  private pad(): Padding {
    if (this.compact) {
      return { top: 32, bottom: 32, left: 24, right: 24 };
    }
    return { left: 32, right: 32 };
  }

  private chipSize(): number {
    return this.compact ? 44 : 60;
  }

  build() {
    Column({ space: this.compact ? 8 : 12 }) {
      if (this.spin) {
        LoadingProgress()
          .width(this.compact ? 30 : 40)
          .height(this.compact ? 30 : 40)
          .color(this.tk().accent)
      } else if (this.iconRes !== undefined) {
        // Round icon chip: subtle base + border + soft shadow (GitHub blankslate).
        Stack() {
          Icon({ src: this.iconRes, iconSize: this.compact ? 20 : 26, color: this.tk().textSecondary })
        }
        .width(this.chipSize())
        .height(this.chipSize())
        .borderRadius(this.chipSize() / 2)
        .radialGradient({
          center: ['50%', '38%'],
          radius: this.chipSize(),
          colors: [[this.tk().bgCardHover, 0.0], [this.tk().bgCard, 1.0]],
        })
        .border({ width: 1, color: this.tk().border })
        .shadow({ radius: 8, color: 'rgba(0,0,0,0.12)', offsetX: 0, offsetY: 2 })
      } else if (this.icon.length > 0) {
        Text(this.icon).fontSize(this.compact ? 34 : 46)
      }
      Text(this.title)
        .fontSize(this.compact ? 14 : 15)
        .fontWeight(600)
        .fontColor(this.tk().text)
        .textAlign(TextAlign.Center)
      if (this.subtitle.length > 0) {
        Text(this.subtitle)
          .fontSize(13)
          .fontColor(this.tk().textSecondary)
          .textAlign(TextAlign.Center)
          .lineHeight(19)
      }
    }
    .width('100%')
    .height(this.compact ? 'auto' : '100%')
    .padding(this.pad())
    .justifyContent(FlexAlign.Center)
    .opacity(this.appeared)
  }
}
