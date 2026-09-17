import { webview } from '@kit.ArkWeb';
import { common } from '@kit.AbilityKit';
import { RelayStore, RelayPairing } from '../common/RelayStore';
import { saveBase64File } from '../common/FileTransfer';
import { setKeepScreenOn } from '../common/WakeLock';
import { shareFile } from '../common/SystemShare';
import { acquirePushToken } from '../common/PushRegister';
import { SpeechAsr } from '../common/SpeechAsr';
import { scanBarcode, scanCore } from '@kit.ScanKit';
import { parseScan, DirectHostScan } from '../common/RelayStore';
import { themeTokens } from '../common/Theme';
import { hilog } from '@kit.PerformanceAnalysisKit';
import { fileIo as fs } from '@kit.CoreFileKit';

const DOMAIN_W = 0x0000;
const TAG_W = 'fleet';

/// Harmony-side WebView shell: runs mobile-web bundled into rawfile; native handles only
/// three things the web can't (pairing key injection, safe area, push notifications).
///
/// Why a fake domain instead of resource://rawfile/ — the latter loads HTML fine and
/// resolves relative paths correctly, but subresources fail and onInterceptRequest doesn't
/// intercept built-in schemes (both verified on real devices, not from docs). HTTPS not HTTP
/// is also mandatory: mobile-web's relayCrypto depends on crypto.subtle, which only works
/// in secure contexts.
const ORIGIN = 'https://fleet.local/';
/// Incoming shared files mounted under this prefix, separated from site resources in rawfile.
const SHARED_PREFIX = '__shared/';

/// File entry passed to web, field names aligned with shareTarget.ts::SharedFile.
interface SharedFileEntry {
  uri: string;
  name: string;
  mimeType: string;
}

/// Payload for __fleetShare, shape aligned with shareTarget.ts::IncomingShare.
interface SharePayload {
  title: string;
  texts: string[];
  files: SharedFileEntry[];
}

/// Payload for __fleetVoice, shape aligned with voiceHarmony.ts::HarmonyVoiceEvent.
/// Five event types on one channel: ready, partial result, final, error, engine stop.
interface VoiceEvent {
  kind: string;
  text: string;
  code: string;
}


/// Native bridge exposed to the page. Only include what the web truly cannot do in WebView—
/// every added method pressures mobile-web to write Harmony branches, exactly the drift we want to eliminate.
class NativeBridge {
  private ctx: common.UIAbilityContext;
  /// Callback when page requests barcode scan; updates component state (src / pairError).
  private onScanRequested: () => void;
  /// Speech recognition event exit. Passed in by page component — events go back to the page
  /// via controller.runJavaScript, and the controller lives on the component.
  private onVoiceEvent: (json: string) => void;
  /// Result exit for re-authorization. Like onVoiceEvent: goes back via controller.runJavaScript,
  /// controller lives on the component.
  private onVoicePermission: (granted: boolean) => void;
  private asr: SpeechAsr = new SpeechAsr();

  constructor(
    ctx: common.UIAbilityContext,
    onScanRequested: () => void,
    onVoiceEvent: (json: string) => void,
    onVoicePermission: (granted: boolean) => void,
  ) {
    this.ctx = ctx;
    this.onScanRequested = onScanRequested;
    this.onVoiceEvent = onVoiceEvent;
    this.onVoicePermission = onVoicePermission;
  }

  /// <a download> / createObjectURL in WebView don't trigger system downloads;
  /// use DocumentViewPicker to let the user choose save location.
  saveFile(filename: string, base64: string): void {
    saveBase64File(this.ctx, filename, base64).catch(() => {});
  }

  /// Trigger barcode scan to add device. The shell's pairing page is only reachable when
  /// **not yet paired** (build() only draws it when src is empty), so installed users have no
  /// second way to add a second device—which is the main multi-device entry. Here we expose
  /// scan to the page, which renders "Scan to add device" in its device list (mobile-web::nativeScan.ts).
  ///
  /// Callback runs on the next WebView reload: after scan, call loadPairing(true) to inject
  /// a new `#k=`; the web side treats it as a new device as usual.
  scanPairing(): void {
    this.onScanRequested();
  }

  /// navigator.wakeLock may not be available in ArkWeb; page falls back to this when it's not.
  keepAwake(on: boolean): void {
    setKeepScreenOn(this.ctx, on);
  }

  /// Native side of navigator.share, calls ShareKit to bring up system share sheet.
  shareContent(filename: string, mimeType: string, base64: string): void {
    shareFile(this.ctx, filename, mimeType, base64).catch(() => {});
  }

  /// Native side of voice input (voiceHarmony.ts in mobile-web). Web Speech API is unavailable
  /// in ArkWeb, so the page has no recognition capability and must rely on the shell.
  ///
  /// Results do **not go through return values**: recognition is streaming (one utterance yields
  /// multiple interim results and one final), but javaScriptProxy methods are request-response.
  /// Here we only handle "start"; results flow back to the page via window.__fleetVoice—same
  /// contract as __fleetShare / __fleetPushToken, except no bootScript placeholder needed:
  /// voice events only happen after the user presses a button, when the page hook is already
  /// registered, so there's no cold-start race.
  startVoice(lang: string): void {
    this.asr.start(this.ctx, lang, {
      onReady: () => {
        const ev: VoiceEvent = { kind: 'ready', text: '', code: '' };
        this.onVoiceEvent(JSON.stringify(ev));
      },
      onText: (text: string, isFinal: boolean) => {
        const ev: VoiceEvent = { kind: isFinal ? 'final' : 'partial', text: text, code: '' };
        this.onVoiceEvent(JSON.stringify(ev));
      },
      onError: (code: string) => {
        const ev: VoiceEvent = { kind: 'error', text: '', code: code };
        this.onVoiceEvent(JSON.stringify(ev));
      },
      onComplete: () => {
        // Engine stops on its own (VAD detects end-of-speech or maxAudioDuration reached).
        // Must release resources or microphone stays locked; page resets button based on this.
        this.asr.release();
        const ev: VoiceEvent = { kind: 'end', text: '', code: '' };
        this.onVoiceEvent(JSON.stringify(ev));
      },
    }).catch(() => {
      const ev: VoiceEvent = { kind: 'error', text: '', code: 'START_FAILED' };
      this.onVoiceEvent(JSON.stringify(ev));
    });
  }

  /// Stop recording but let engine emit final result for the last segment.
  stopVoice(): void {
    this.asr.stop();
  }

  /// Discard this recognition attempt.
  cancelVoice(): void {
    this.asr.cancel();
  }

  /// Re-authorization after user denies microphone (web side: voiceHarmony.openPermissionSettings).
  ///
  /// The "no microphone permission" message on-page was a dead-end: requestPermissionsFromUser
  /// won't pop again after denial, and the page has no way to take the user to settings. This
  /// method brings up the system permission panel; result flows back to the page via
  /// window.__fleetVoicePermission, which then starts recording directly—user needs one action
  /// per "I want to speak" intent.
  openVoiceSettings(): void {
    this.asr.requestOnSetting(this.ctx).then((granted: boolean) => {
      this.onVoicePermission(granted);
    }).catch(() => {
      this.onVoicePermission(false);
    });
  }
}

@Entry
@Component
struct WebShell {
  private controller: webview.WebviewController = new webview.WebviewController();
  /// Safe area published to AppStorage by SafeArea.ets; mobile-web abstracts it as
  /// --safe-top / --safe-bottom CSS variables (default: env(safe-area-inset-*), but WebView
  /// env can't get system values). Native just overrides these two variables; web needs no changes.
  @StorageProp('safeTop') safeTop: number = 0;
  @StorageProp('safeBottom') safeBottom: number = 0;
  /// Web component defaults to white background. The instant before the page renders and any
  /// uncovered corners show this white—especially jarring in dark theme. Follow resolvedTheme
  /// to provide a matching backdrop.
  @StorageProp('resolvedTheme') resolvedTheme: string = 'dark';
  /// EntryAbility puts system share content here after receiving it; nonce change means
  /// "shared again," even if the text is identical. @Watch ensures shares while app is open get delivered.
  @StorageProp('sharedText') sharedText: string = '';
  /// Incoming share filenames (newline-separated). SharePanel copies bytes into sandbox;
  /// we just build __shared/ URLs and pass them to web; onInterceptRequest retrieves bytes.
  @StorageProp('sharedFiles') sharedFiles: string = '';
  @StorageProp('sharedNonce') @Watch('onSharedChanged') sharedNonce: number = 0;
  /// Navigation target from tapping a push notification (EntryAbility reads from want,
  /// see takeDeepLinkFromWant). Nonce change means "tapped again," even if target is the same.
  @StorageProp('deepLinkUrl') deepLinkUrl: string = '';
  @StorageProp('deepLinkNonce') @Watch('onDeepLinkChanged') deepLinkNonce: number = 0;
  private webReady: boolean = false;
  /// Device push token, cached once obtained—getToken brings up system notification auth,
  /// repeating is pointless. Empty string means not yet fetched or unavailable on this device
  /// (simulator / push not enabled).
  private pushToken: string = '';
  @State private src: string = '';
  /// null = still reading pairing state; '' = not paired (show pairing entry); else = paired src.
  /// Three states because "not yet read" and "read but empty" must be distinguished—otherwise
  /// cold start would flash the pairing page then navigate away.
  @State private pairChecked: boolean = false;
  @State private pairError: string = '';
  private bridge: NativeBridge = new NativeBridge(
    getContext(this) as common.UIAbilityContext,
    () => { this.startPairing(); },
    (json: string) => { this.deliverVoiceEvent(json); },
    (granted: boolean) => { this.deliverVoicePermission(granted); },
  );

  aboutToAppear(): void {
    this.loadPairing();
  }

  /// `fresh` = just scanned a code (take focus on that device); default is startup re-inject (don't take focus).
  private loadPairing(fresh: boolean = false): void {
    const ctx = getContext(this) as common.UIAbilityContext;
    RelayStore.loadPairing(ctx).then((p: RelayPairing | null) => {
      this.pairChecked = true;
      // Secret in URL fragment: mobile-web's consumeHashSecret already reads #k= from
      // window.location.hash (desktop QR uses this shape), so we reuse the same path and
      // web needs no Harmony branch. Fragment doesn't enter the request line, so the interceptor
      // fetching by path works fine.
      // Leave src empty when unpaired to let build() show the native pairing page.
      //
      // Early on we also sent ORIGIN + 'index.html' when p === null, but that left src
      // non-empty forever, so the pairing page never opened—the page got stuck on mobile-web's
      // PWA bootstrap (which assumes user opened a QR-scanned link with a key, so no scan button).
      // This eliminated all pairing entry points.
      //
      // Also pass relayUrl to the page. Previously we only fetched secret; relay always came
      // from VITE_RELAY_URL baked in by sync-web.sh at build time. Scan a QR for self-hosted
      // relay, app still connects to the official one, pairing fails silently with "won't connect."
      // mobile-web's consumeHashSecret reads &relay= in the fragment and stores it with the key
      // in that device's record (devices.ts), higher priority than baked-in value. We send it here,
      // it takes effect, web needs no Harmony branch. After multi-device, relay belongs to **each
      // device individually**; this shell re-injects on every load, so self-hosted relay heals after upgrades.
      // boot=1 means **startup re-inject**, not a freshly scanned code. After multi-device, web stores
      // pairing as a device ledger and shell re-injects each startup. Without this distinction, a user
      // who switched to another device in the list gets pulled back on every app restart—unwanted focus theft.
      // Fresh scan uses loadPairing(true) without this flag—then taking focus is what the user expects.
      const boot = fresh ? '' : '&boot=1';
      this.src = p === null
        ? ''
        : ORIGIN + 'index.html#k=' + p.secret + '&relay=' + encodeURIComponent(p.relayUrl) + boot;
    }).catch(() => { this.pairChecked = true; this.src = ''; });
  }

  /// Pairing entry point. When we removed native UI, we deleted GateView with it; mobile-web's
  /// pairing page is written for PWA—it assumes "user opened a QR-scanned URL with a key."
  /// The shell has no such entry, so new app installs have no way to pair. Here we add back the
  /// minimum: scan barcode, write to RelayStore, reload WebView with #k=.
  private async startPairing(): Promise<void> {
    const ctx = getContext(this) as common.UIAbilityContext;
    try {
      const options: scanBarcode.ScanOptions = {
        scanTypes: [scanCore.ScanType.QR_CODE],
        enableMultiMode: false,
        enableAlbum: true,
      };
      const result = await scanBarcode.startScanForResult(ctx, options);
      const scan = parseScan(result.originalValue);
      // Direct-host code: shell doesn't persist it—page maintains its own device ledger.
      // Shell passes the raw scan result once to the page (#h=…&t=…), page probes then enrols.
      // If already paired, add to that page's current ledger; if not, start with just this device
      // (no relay device yet, but direct connect to this one works).
      if (scan.host !== null) {
        const host: DirectHostScan = scan.host;
        this.pairError = '';
        this.src = ORIGIN + 'index.html#h=' + encodeURIComponent(host.baseUrl)
          + '&t=' + encodeURIComponent(host.token);
        return;
      }
      if (scan.pairing === null) {
        this.pairError = '这个二维码不是 Fleet 的配对码';
        return;
      }
      await RelayStore.savePairing(ctx, scan.pairing);
      this.pairError = '';
      this.loadPairing(true);
    } catch (e) {
      this.pairError = '扫码取消或失败';
    }
  }

  private onSharedChanged(): void {
    this.deliverShare(0);
  }

  private onDeepLinkChanged(): void {
    this.deliverDeepLink();
  }

  /// Pass tap target to the page via decisionDeepLink.ts contract: window.__fleetDeepLink.
  /// Same contract as __fleetShare / __fleetPushToken; bootScript pre-registers and queues,
  /// so cold start (tapping a notification) doesn't race React effects.
  private deliverDeepLink(): void {
    if (!this.webReady || this.deepLinkUrl.length === 0) {
      hilog.info(DOMAIN_W, TAG_W, 'deeplink-diag deliver skipped ready=%{public}s len=%{public}d',
        this.webReady ? 'yes' : 'no', this.deepLinkUrl.length);
      return;
    }
    this.controller.runJavaScript(
      `(function(){ if(!window.__fleetDeepLink) return 'NO_HOOK';` +
      ` window.__fleetDeepLink(` + JSON.stringify(this.deepLinkUrl) + `); return 'OK'; })()`,
    ).then((r: string) => {
      hilog.info(DOMAIN_W, TAG_W, 'deeplink-diag injected result=%{public}s', r);
      if (r.indexOf('OK') >= 0) {
        // Clear on success to avoid jumping the same card again when returning to foreground.
        this.deepLinkUrl = '';
      }
    }).catch((e: Error) => {
      hilog.warn(DOMAIN_W, TAG_W, 'deeplink-diag inject failed: %{public}s', e.message);
    });
  }

  /// Pass a voice event to the page (voiceHarmony.ts::window.__fleetVoice).
  ///
  /// Unlike deliverShare / deliverDeepLink, we don't need bootScript placeholder or retry on
  /// delivery fail: voice events only happen after the user presses a button, when the page hook
  /// is definitely registered. No hook means the page was swapped mid-recognition—that result
  /// should be discarded anyway.
  ///
  /// **Must log injection result** (like deliverDeepLink): `NO_HOOK` is the only proof of failures
  /// like "page gave up on recognition but native still pushes results." We used to just catch
  /// exceptions and discard returns, so "permission prompt appeared but no text comes out" on real
  /// devices had zero clues—page looked fine, shell looked fine.
  /// Pass re-authorization result to the page (voiceHarmony.ts::window.__fleetVoicePermission).
  ///
  /// Log it too: delivery fail shows as "tapped to authorize, panel appeared, interface doesn't move,"
  /// and that's exactly when the page waits for this callback.
  private deliverVoicePermission(granted: boolean): void {
    if (!this.webReady) {
      hilog.info(DOMAIN_W, TAG_W, 'voice-diag permission deliver skipped: web not ready');
      return;
    }
    this.controller.runJavaScript(
      `(function(){ if(!window.__fleetVoicePermission) return 'NO_HOOK';` +
      ` window.__fleetVoicePermission(` + (granted ? 'true' : 'false') + `); return 'OK'; })()`,
    ).then((r: string) => {
      hilog.info(DOMAIN_W, TAG_W, 'voice-diag permission injected granted=%{public}s result=%{public}s',
        granted ? 'true' : 'false', r);
    }).catch((e: Error) => {
      hilog.warn(DOMAIN_W, TAG_W, 'voice-diag permission deliver failed: %{public}s', e.message);
    });
  }

  private deliverVoiceEvent(json: string): void {
    if (!this.webReady) {
      hilog.info(DOMAIN_W, TAG_W, 'voice-diag deliver skipped: web not ready');
      return;
    }
    this.controller.runJavaScript(
      `(function(){ if(!window.__fleetVoice) return 'NO_HOOK';` +
      ` window.__fleetVoice(` + json + `); return 'OK'; })()`,
    ).then((r: string) => {
      hilog.info(DOMAIN_W, TAG_W, 'voice-diag injected result=%{public}s', r);
    }).catch((e: Error) => {
      hilog.warn(DOMAIN_W, TAG_W, 'voice-diag deliver failed: %{public}s', e.message);
    });
  }

  /// Pass share content to the page via shareTarget.ts contract: window.__fleetShare—a generic
  /// entry for all native shells, not Harmony-specific; Capacitor uses it too.
  ///
  /// No retry: bootScript pre-registers __fleetShare before any page script, early arrivals queue
  /// in __fleetSharePending, consumed when shareTarget.ts registers the real hook. Timing dependency gone.
  ///
  /// Why no retry: real device logs show only attempt=0; setTimeout in ArkTS struct methods
  /// doesn't callback at all—that retry code was dead.
  private deliverShare(attempt: number): void {
    if (!this.webReady || (this.sharedText.length === 0 && this.sharedFiles.length === 0)) {
      hilog.info(DOMAIN_W, TAG_W, 'share-diag deliver skipped ready=%{public}s len=%{public}d',
        this.webReady ? 'yes' : 'no', this.sharedText.length);
      return;
    }
    const fileEntries: SharedFileEntry[] = [];
    for (const name of this.sharedFiles.split('\n')) {
      if (name.length === 0) { continue; }
      // Give web a URL it can fetch directly. shareTarget.ts::sharedFilesToFiles fetches it
      // → interceptor returns bytes from sandbox → becomes File → goes through existing
      // attachment upload flow. Web needs no changes.
      const entry: SharedFileEntry = {
        uri: ORIGIN + SHARED_PREFIX + name,
        name: name,
        mimeType: this.mime(name),
      };
      fileEntries.push(entry);
    }
    const share: SharePayload = {
      title: '',
      texts: this.sharedText.length > 0 ? [this.sharedText] : [],
      files: fileEntries,
    };
    const payload: string = JSON.stringify(share);
    this.controller.runJavaScript(
      `(function(){ if(!window.__fleetShare) return 'NO_HOOK';` +
      ` window.__fleetShare(` + payload + `); return 'OK'; })()`,
    ).then((r: string) => {
      const ok: boolean = r.indexOf('OK') >= 0;
      hilog.info(DOMAIN_W, TAG_W, 'share-diag injected attempt=%{public}d result=%{public}s',
        attempt, r);
      if (ok) {
        // Clear after delivery to avoid re-posting the same content on next foreground.
        this.sharedText = '';
        this.sharedFiles = '';
        return;
      }
      // Reach here = no placeholder hook even (bootScript didn't run). Different problem,
      // retry can't help—logging beats spinning.
      hilog.warn(DOMAIN_W, TAG_W, 'share-diag no hook at all — bootScript may not have run');
    }).catch((e: Error) => {
      hilog.warn(DOMAIN_W, TAG_W, 'share-diag inject failed: %{public}s', e.message);
    });
  }

  /// Fetch device push token and pass to page. Native just "gets the token"; reporting to relay
  /// is web's job (nativePush.ts → push.ts)—it holds the relay connection, channel token, and
  /// user notification settings. In the Element Services era, native opened WebSocket to report openId,
  /// requiring a separate HKDF implementation in ArkTS; both implementations drifted and we deleted them with this refactor.
  ///
  /// Only when paired: unpaired web has no channel to register. Prompting for notification auth
  /// early just annoys the user.
  private deliverPushToken(): void {
    if (!this.webReady) { return; }
    if (this.pushToken.length > 0) {
      this.injectPushToken();
      return;
    }
    const ctx = getContext(this) as common.UIAbilityContext;
    acquirePushToken(ctx).then((t: string) => {
      if (t.length === 0) {
        hilog.info(DOMAIN_W, TAG_W, 'push-diag no token on this device — skip register');
        return;
      }
      this.pushToken = t;
      this.injectPushToken();
    }).catch((e: Error) => {
      hilog.warn(DOMAIN_W, TAG_W, 'push-diag acquire failed: %{public}s', e.message);
    });
  }

  /// Same contract as __fleetShare: bootScript pre-registers and queues, so no retry needed
  /// and can't use setTimeout (ArkTS struct methods' setTimeout doesn't callback—share already
  /// buried us in that pit once).
  private injectPushToken(): void {
    this.controller.runJavaScript(
      `(function(){ if(!window.__fleetPushToken) return 'NO_HOOK';` +
      ` window.__fleetPushToken(` + JSON.stringify(this.pushToken) + `); return 'OK'; })()`,
    ).then((r: string) => {
      hilog.info(DOMAIN_W, TAG_W, 'push-diag injected len=%{public}d result=%{public}s',
        this.pushToken.length, r);
    }).catch((e: Error) => {
      hilog.warn(DOMAIN_W, TAG_W, 'push-diag inject failed: %{public}s', e.message);
    });
  }

  /// Read an incoming shared file from sandbox and feed to WebView.
  private serveSharedFile(name: string): WebResourceResponse | null {
    try {
      const ctx = getContext(this) as common.UIAbilityContext;
      const path: string = ctx.filesDir + '/shared/' + name;
      const stat = fs.statSync(path);
      const buf = new ArrayBuffer(stat.size);
      const file = fs.openSync(path, fs.OpenMode.READ_ONLY);
      try {
        fs.readSync(file.fd, buf);
      } finally {
        fs.closeSync(file);
      }
      const resp = new WebResourceResponse();
      resp.setResponseData(buf);
      resp.setResponseMimeType(this.mime(name));
      resp.setResponseEncoding('utf-8');
      resp.setResponseCode(200);
      resp.setReasonMessage('OK');
      resp.setResponseIsReady(true);
      hilog.info(DOMAIN_W, TAG_W, 'share-diag served %{public}s bytes=%{public}d', name, stat.size);
      return resp;
    } catch (e) {
      const err = e as Error;
      hilog.warn(DOMAIN_W, TAG_W, 'share-diag serve failed %{public}s: %{public}s', name, err.message);
      return null;
    }
  }

  private mime(path: string): string {
    if (path.endsWith('.js') || path.endsWith('.mjs')) return 'application/javascript';
    if (path.endsWith('.css')) return 'text/css';
    if (path.endsWith('.html')) return 'text/html';
    if (path.endsWith('.json') || path.endsWith('.webmanifest')) return 'application/json';
    if (path.endsWith('.svg')) return 'image/svg+xml';
    if (path.endsWith('.png')) return 'image/png';
    if (path.endsWith('.jpg') || path.endsWith('.jpeg')) return 'image/jpeg';
    if (path.endsWith('.gif')) return 'image/gif';
    if (path.endsWith('.webp')) return 'image/webp';
    if (path.endsWith('.pdf')) return 'application/pdf';
    if (path.endsWith('.ico')) return 'image/x-icon';
    if (path.endsWith('.woff2')) return 'font/woff2';
    if (path.endsWith('.woff')) return 'font/woff';
    return 'application/octet-stream';
  }

  private strip(s: string): string {
    let r: string = s;
    const q: number = r.indexOf('?');
    if (q >= 0) r = r.substring(0, q);
    const h: number = r.indexOf('#');
    if (h >= 0) r = r.substring(0, h);
    return r;
  }

  /// Run before any page script: safe area must inject before first frame (else visible jank),
  /// patches too must run before page code registers. All runtime patches; mobile-web source unchanged.
  private bootScript(): string {
    return `(function(){
      // Pre-register __fleetShare so delivery doesn't race React's useEffect.
      //
      // We used to rely on "retry with setTimeout if inject fails"; real logs show only attempt=0—
      // setTimeout in ArkTS struct methods never callbacks, retry was pointless. Placeholder hook
      // queues content, consumed once when shareTarget.ts registers the real hook; timing dependency gone.
      if (!window.__fleetShare) {
        window.__fleetSharePending = window.__fleetSharePending || [];
        window.__fleetShare = function(p){ window.__fleetSharePending.push(p); };
      }

      // Push token placeholder same idea: native fetches token after system auth prompt, always
      // after first frame; nativePush.ts hook hangs on React effect—both timings uncertain,
      // so both queue.
      if (!window.__fleetPushToken) {
        window.__fleetPushTokenPending = window.__fleetPushTokenPending || [];
        window.__fleetPushToken = function(t){ window.__fleetPushTokenPending.push(t); };
      }

      // Tap notification to launch app: target arrives before page script—also pre-register and queue.
      if (!window.__fleetDeepLink) {
        window.__fleetDeepLinkPending = window.__fleetDeepLinkPending || [];
        window.__fleetDeepLink = function(u){ window.__fleetDeepLinkPending.push(u); };
      }

      var s = document.documentElement.style;
      // Only set bottom. Top is already inset by the framework—SafeArea.ets comment records it:
      // on 2026-07-15 app added top inset again, pushing every page header down a line.
      // My first version blindly copied safeTop injection, reintroducing that fixed bug.
      // safeTop still publishes to AppStorage for diagnostics, just not fed to the page.
      s.setProperty('--safe-bottom', '` + this.safeBottom.toString() + `px');

      // Download: intercept a[download] clicks, read blob/data as base64, pass to native to save.
      // Use capture phase to run before page's own handler.
      document.addEventListener('click', function(ev){
        var a = ev.target && ev.target.closest ? ev.target.closest('a[download]') : null;
        if (!a || !a.href) return;
        ev.preventDefault();
        ev.stopPropagation();
        fetch(a.href).then(function(r){ return r.blob(); }).then(function(b){
          var fr = new FileReader();
          fr.onload = function(){
            var s = String(fr.result || '');
            var i = s.indexOf(',');
            if (i >= 0 && window.fleetNative) {
              window.fleetNative.saveFile(a.getAttribute('download') || 'download', s.slice(i+1));
            }
          };
          fr.readAsDataURL(b);
        }).catch(function(){});
      }, true);

      // System share: polyfill Web Share with ShareKit call when ArkWeb lacks it.
      if (!navigator.share && window.fleetNative) {
        navigator.canShare = function(d){ return !!(d && d.files && d.files.length); };
        navigator.share = function(d){
          var f = d && d.files && d.files[0];
          if (!f) return Promise.reject(new Error('nothing to share'));
          return new Promise(function(res, rej){
            var fr = new FileReader();
            fr.onload = function(){
              var s = String(fr.result || '');
              var i = s.indexOf(',');
              if (i < 0) { rej(new Error('bad data url')); return; }
              window.fleetNative.shareContent(f.name || 'share', f.type || '', s.slice(i+1));
              res();
            };
            fr.onerror = function(){ rej(fr.error); };
            fr.readAsDataURL(f);
          });
        };
      }

      // Keep awake: if ArkWeb lacks navigator.wakeLock, polyfill with native call,
      // so wakeLock.ts code doesn't need to know it's running on Harmony.
      if (!navigator.wakeLock && window.fleetNative) {
        navigator.wakeLock = {
          request: function(){
            window.fleetNative.keepAwake(true);
            return Promise.resolve({
              released: false,
              type: 'screen',
              release: function(){ window.fleetNative.keepAwake(false); return Promise.resolve(); },
              addEventListener: function(){},
              removeEventListener: function(){}
            });
          }
        };
      }
    })();`;
  }

  /// System back gesture doesn't trigger page popstate—without this, back exits the app instead of
  /// popping one layer. mobile-web's navStack pushState on every layer open, so WebView history
  /// depth equals UI layer count.
  onBackPress(): boolean {
    if (this.controller.accessBackward()) {
      this.controller.backward();
      return true; // Already consumed, do not exit app
    }
    return false; // Already at root level, return to system
  }

  build() {
    Column() {
      if (this.pairChecked && this.src.length === 0) {
        Column({ space: 16 }) {
          Text('Fleet').fontSize(28).fontWeight(FontWeight.Bold)
          Text('扫描桌面端 Fleet「移动端」面板里的二维码完成配对')
            .fontSize(14).opacity(0.7).textAlign(TextAlign.Center)
          Button('扫码配对').onClick(() => { this.startPairing(); })
          if (this.pairError.length > 0) {
            Text(this.pairError).fontSize(12).fontColor(Color.Red)
          }
        }
        .width('100%').height('100%')
        .justifyContent(FlexAlign.Center)
        .padding(32)
      } else if (this.src.length > 0) {
        Web({ src: this.src, controller: this.controller })
          .backgroundColor(themeTokens(this.resolvedTheme).bg)
          .javaScriptAccess(true)
          .domStorageAccess(true)
          .fileAccess(true)
          .imageAccess(true)
          .databaseAccess(true)
          .mixedMode(MixedMode.All)
          .darkMode(WebDarkMode.Auto)
          // JS alerts must be handled by native. ArkWeb doesn't draw alert/confirm dialogs:
          // returning false from callback means "treat as cancelled"; no callback = same.
          // So `window.confirm(...)` always returns false, `window.alert(...)` silently vanishes—
          // Task page "Stop" button unresponsive because its confirm goes this route.
          //
          // After registering, must call handleConfirm / handleCancel; missing it blocks render
          // thread. All three exits (OK, Cancel, tap outside/back key) must call result.
          //
          // Skip window.prompt: mobile-web never uses it (grepped), and it needs custom input
          // dialog, not just AlertDialog. Add if needed.
          .onAlert((event) => {
            if (!event) return false;
            this.getUIContext().showAlertDialog({
              message: event.message,
              autoCancel: false,
              confirm: {
                value: '知道了',
                action: () => { event.result.handleConfirm(); },
              },
              cancel: () => { event.result.handleCancel(); },
            });
            return true;
          })
          .onConfirm((event) => {
            if (!event) return false;
            this.getUIContext().showAlertDialog({
              message: event.message,
              autoCancel: false,
              primaryButton: {
                value: '取消',
                action: () => { event.result.handleCancel(); },
              },
              secondaryButton: {
                value: '确定',
                action: () => { event.result.handleConfirm(); },
              },
              cancel: () => { event.result.handleCancel(); },
            });
            return true;
          })
          // Pass outlinks to system browser, don't let them hijack the whole app.
          //
          // multiWindowAccess defaults false; ArkWeb then treats target="_blank" as
          // "open local window"—click an external link in wiki docs, the only WebView
          // navigates away from fleet.local, Fleet UI and relay connection disappear,
          // only back key recovers. Enabling multiWindowAccess needs onWindowNew to manage
          // another WebView, not worth it for a few links; hand off non-site main-frame
          // navigations to system.
          //
          // Only intercept main-frame http/https: wiki HTML docs run in data: URI sandbox
          // iframe (intentional by web side), blob: is for export preview—both must pass through.
          .onLoadIntercept((event) => {
            if (!event) return false;
            const req = event.data;
            if (!req.isMainFrame()) return false;
            const url: string = req.getRequestUrl();
            if (url.startsWith(ORIGIN)) return false;
            if (!url.startsWith('http://') && !url.startsWith('https://')) return false;
            try {
              const ctx = getContext(this) as common.UIAbilityContext;
              ctx.openLink(url, { appLinkingOnly: false }).catch((e: Error) => {
                hilog.warn(DOMAIN_W, TAG_W, 'openLink failed %{public}s', e.message);
              });
            } catch (e) {
              hilog.warn(DOMAIN_W, TAG_W, 'openLink threw: %{public}s', (e as Error).message);
            }
            return true;
          })
          .runJavaScriptOnDocumentStart([{ script: this.bootScript(), scriptRules: ['*'] }])
          .javaScriptProxy({
            object: this.bridge,
            name: 'fleetNative',
            methodList: [
              'saveFile', 'keepAwake', 'shareContent', 'scanPairing',
              // Voice input. Web side uses startVoice presence in detectVoiceProvider—
              // old shell with new web: if these three methods don't exist, page auto-fallsback,
              // won't call non-existent implementations.
              'startVoice', 'stopVoice', 'cancelVoice', 'openVoiceSettings',
            ],
            controller: this.controller,
          })
          .onInterceptRequest((event) => {
            const url: string = event.request.getRequestUrl();
            if (!url.startsWith(ORIGIN)) return null;
            const tail: string = this.strip(url.substring(ORIGIN.length));
            // Incoming shared files aren't in rawfile, but in app sandbox files/shared/.
            if (tail.startsWith(SHARED_PREFIX)) {
              return this.serveSharedFile(tail.substring(SHARED_PREFIX.length));
            }
            const rel: string = 'web/' + tail;
            try {
              const ctx = getContext(this) as common.UIAbilityContext;
              const bytes: Uint8Array = ctx.resourceManager.getRawFileContentSync(rel);
              const resp = new WebResourceResponse();
              resp.setResponseData(bytes.buffer as ArrayBuffer);
              resp.setResponseMimeType(this.mime(rel));
              resp.setResponseEncoding('utf-8');
              resp.setResponseCode(200);
              resp.setReasonMessage('OK');
              resp.setResponseIsReady(true);
              return resp;
            } catch (e) {
              return null;
            }
          })
          .onPageEnd(() => {
            // Re-deliver after page ready: share likely came in cold start when web wasn't
            // loaded; @Watch already fired.
            this.webReady = true;
            this.deliverShare(0);
            this.deliverPushToken();
            this.deliverDeepLink();
          })
          .width('100%')
          .height('100%')
      }
    }
    .width('100%')
    .height('100%')
    // Expand only down to the gesture bar. ArkUI components default-avoid safe area, so WebView's
    // bottom stops at gesture bar top; that gap shows window background (white), not webpage—no
    // CSS can reach it, hence "nav bar gray, bottom edge white." After expanding, webpage fills
    // to screen bottom; --safe-bottom padding keeps nav bar content clear of gesture bar.
    //
    // Don't expand top: top frame already insets (see SafeArea.ets 2026-07-15 note);
    // expanding would push page header below status bar.
    .expandSafeArea([SafeAreaType.SYSTEM], [SafeAreaEdge.BOTTOM])
  }
}
