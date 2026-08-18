/// Renders the fleet__ask `html` preview field with the ArkWeb `Web` component.
/// Despite the long-standing "atomic services can't use Web" lore, an on-device
/// spike (HarmonyOS 6.1.1, real device) proved the ArkWeb runtime loads and
/// paints inside this atomic service with no restriction — so we render the
/// agent-provided HTML for real instead of degrading to a "view on desktop" note.
///
/// Contract match with mobile-web: the desktop shows this html in an
/// `<iframe sandbox="">` (no scripts, no same-origin). We mirror that by loading
/// JavaScript-disabled, with our own theme-coloured wrapper stylesheet injected
/// so tables / code / quotes read consistently in light and dark. The Web sizes
/// to its content via `WebLayoutMode.FIT_CONTENT` (ArkWeb's auto-height mode) so
/// it never traps card scroll gestures behind an inner scroller — a manual
/// getPageHeight() + fixed initial height deadlocks (a 1px-tall Web never paints,
/// so onPageEnd never fires to grow it), which an on-device spike confirmed.
///
/// Images: `<img src="name">` refs (served over fleet-decision:// on the desktop)
/// are resolved by pre-fetching each named asset through the same `decision_asset`
/// relay call the image gallery uses, then inlining it as a `data:` URI — so the
/// loaded document is self-contained. `onlineImageAccess` is left at its default
/// (on): an on-device spike showed disabling it also blocks `data:` images, and
/// leaving it on matches the desktop `<iframe sandbox>`, which loads images too
/// (sandbox blocks scripts, not image loads). JavaScript stays disabled.

import webview from '@ohos.web.webview';
import { ColorTokens, themeTokens } from './Theme';
import { RelayClient } from './Relay';

/// decision_asset request/response — same relay call DecisionImage uses to pull
/// a card's image bytes by relative name (mirrored here rather than shared, since
/// the interfaces live privately in DecisionsView).
interface AssetParams {
  id: string;
  qidx: string;
  rel: string;
}
interface AssetResp {
  mime: string;
  base64: string;
}

@Component
export struct HtmlView {
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  @Prop html: string = '';
  /// Card coordinates so `<img src="name">` refs can be resolved through the same
  /// `decision_asset` relay call the standalone image gallery uses. Optional —
  /// when absent (or the html has no img refs) the html renders as-is.
  requestId: string = '';
  qidx: number = 0;
  client: RelayClient | null = null;
  /// The html actually loaded: starts as the raw html (so text/tables paint
  /// immediately) and is re-set once img refs are rewritten to data URIs.
  @State private resolvedHtml: string = '';
  private controller: webview.WebviewController = new webview.WebviewController();

  aboutToAppear(): void {
    this.resolvedHtml = this.html;
    this.resolveImages();
  }

  private tk(): ColorTokens {
    return themeTokens(this.resolvedTheme);
  }

  /// Bare `<img src>` names in the html (skipping already-inline data: URIs and
  /// external http(s) URLs, which the sandbox contract doesn't fetch anyway).
  private extractImgNames(html: string): string[] {
    const names: string[] = [];
    const re = new RegExp('<img[^>]*\\ssrc=["\\\']([^"\\\']+)["\\\']', 'gi');
    let m: RegExpExecArray | null = re.exec(html);
    while (m !== null) {
      const v = m[1];
      if (!v.startsWith('data:') && !v.startsWith('http://') && !v.startsWith('https://') && !names.includes(v)) {
        names.push(v);
      }
      m = re.exec(html);
    }
    return names;
  }

  /// Fetch each referenced image via decision_asset and inline it as a data URI.
  /// A failed fetch leaves the bare ref (broken img) — the card's image gallery
  /// below still shows every image, so nothing is lost.
  private async resolveImages(): Promise<void> {
    const client = this.client;
    if (!client) {
      return;
    }
    const names = this.extractImgNames(this.html);
    if (names.length === 0) {
      return;
    }
    let out = this.html;
    for (const name of names) {
      try {
        const params: AssetParams = { id: this.requestId, qidx: `q${this.qidx}`, rel: name };
        const a = await client.request<AssetResp>('decision_asset', params, 60000);
        const dataUri = `data:${a.mime};base64,${a.base64}`;
        out = out.split(`src="${name}"`).join(`src="${dataUri}"`);
        out = out.split(`src='${name}'`).join(`src='${dataUri}'`);
      } catch (e) {
        // leave the bare ref; the gallery below is the fallback source of truth
      }
    }
    this.resolvedHtml = out;
  }

  /// Wrap the raw html in a full document with a viewport tag and a theme-aware
  /// stylesheet. Only solid 6-digit theme tokens are used in CSS — the 8-digit
  /// `border`/`borderStrong` tokens are ArkUI #AARRGGBB, which CSS would read as
  /// #RRGGBBAA and mis-colour, so borders use the solid `textDim` instead.
  private wrap(): string {
    const tk = this.tk();
    const style = `<style>`
      + `html,body{margin:0;padding:0;background:transparent;color:${tk.text};`
      + `font-family:-apple-system,system-ui,sans-serif;font-size:14px;line-height:1.5;`
      + `word-break:break-word;overflow-wrap:break-word;}`
      + `table{border-collapse:collapse;width:100%;margin:4px 0;}`
      + `th,td{border:1px solid ${tk.textDim};padding:4px 8px;text-align:left;}`
      + `th{background:${tk.bgInput};}`
      + `a{color:${tk.accent};}`
      + `img{max-width:100%;height:auto;}`
      + `pre{background:${tk.bgInput};border-radius:4px;padding:8px;overflow-x:auto;}`
      + `code{background:${tk.bgInput};border-radius:4px;padding:1px 4px;}`
      + `pre code{background:transparent;padding:0;}`
      + `blockquote{border-left:3px solid ${tk.accent};margin:6px 0;padding-left:10px;color:${tk.textSecondary};}`
      + `h1,h2,h3{margin:6px 0;}`
      + `</style>`;
    const meta = `<meta name="viewport" content="width=device-width,initial-scale=1">`;
    return `<!DOCTYPE html><html><head>${meta}${style}</head><body>${this.resolvedHtml}</body></html>`;
  }

  private dataUrl(): string {
    return 'data:text/html,' + encodeURIComponent(this.wrap());
  }

  build() {
    Web({ src: this.dataUrl(), controller: this.controller })
      .javaScriptAccess(false)
      .domStorageAccess(false)
      .backgroundColor(Color.Transparent)
      .overScrollMode(OverScrollMode.NEVER)
      .layoutMode(WebLayoutMode.FIT_CONTENT)
      .width('100%')
  }
}
