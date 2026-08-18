/// Native equivalent of mobile-web's lucide-react <Icon>. Renders a bundled
/// lucide SVG (resources/base/media/ic_*.svg — generated from lucide v1.24) and
/// tints it to a theme colour via Image.fillColor, so one asset adapts to
/// light/dark and any accent/status colour.
///
/// Call sites pass the media resource as a literal so ArkUI resolves it at
/// compile time — dynamic `$r('app.media.' + name)` string concatenation is not
/// reliable in strict ArkTS:
///
///   Icon({ src: $r('app.media.ic_search'), size: 15, color: tk.textSecondary })
///
/// `color` empty → defaults to the resolved theme's primary text colour, so most
/// call sites can omit it.

import { themeTokens } from './Theme';

@Component
export struct Icon {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  @Require @Prop src: Resource;
  @Prop iconSize: number = 20;
  @Prop color: string = '';

  build() {
    Image(this.src)
      .width(this.iconSize)
      .height(this.iconSize)
      .fillColor(this.color.length > 0 ? this.color : themeTokens(this.resolvedTheme).text)
      .objectFit(ImageFit.Contain)
      .draggable(false)
  }
}
