/// On-device file transfer for the atomic service client (feature-gaps P5) — attachment
/// selection for the composer and wiki-doc export. Both use DocumentViewPicker, whose `select`
/// and `save` are `@atomicservice` (verified against @ohos.file.picker.d.ts, @since 12), so they
/// work inside the atomic-service sandbox. There is NO outbound ShareKit / systemShare in this SDK
/// (only ShareExtensionAbility for *receiving* shares), so the "share" half of P5 degrades to
/// the platform save dialog — export writes the file to a user-chosen location instead of opening
/// a share sheet.
///
/// ArkTS shape notes: relay params / picker options are declared interfaces or SDK classes
/// (never Record literals); base64 goes through util.Base64Helper; bytes move via the sync
/// fileIo API (files capped at 10 MiB).

import { common } from '@kit.AbilityKit';
import { picker, fileIo as fs, fileUri } from '@kit.CoreFileKit';
import { promptAction } from '@kit.ArkUI';
import { util } from '@kit.ArkTS';

/// 10 MiB — mirrors MAX_UPLOAD_BYTES on the relay (mobile_relay.rs).
const MAX_UPLOAD_BYTES = 10 * 1024 * 1024;

export interface Attachment {
  name: string;
  path: string;
}

interface UploadParams {
  name: string;
  base64: string;
}
interface UploadResp {
  path: string;
}
interface WikiExportParams {
  slug: string;
  version: string;
}
interface WikiExportResp {
  filename: string;
  mime: string;
  base64: string;
}

/// The display / upload name for a picked file URI.
function baseName(uri: string): string {
  try {
    return new fileUri.FileUri(uri).name;
  } catch (_e) {
    const i = uri.lastIndexOf('/');
    return i >= 0 ? uri.substring(i + 1) : uri;
  }
}



/// Save a base64 string as a file at a user-chosen location.
///
/// The latter half extracted from exportWikiDoc: under WebShell, relay connection belongs to the web
/// layer, native cannot access RelayClient, but "let user pick a location and write bytes there" is
/// something only native can do (<a download> in WebView doesn't trigger system download). Web side
/// passes filename and base64 via javaScriptProxy to call this.
export async function saveBase64File(
  context: common.UIAbilityContext,
  filename: string,
  base64: string,
): Promise<boolean> {
  const opts = new picker.DocumentSaveOptions();
  opts.newFileNames = [filename];
  const vp = new picker.DocumentViewPicker(context);
  const uris = await vp.save(opts);
  if (uris.length === 0) {
    return false;
  }
  const bytes = new util.Base64Helper().decodeSync(base64);
  const dst = fs.openSync(uris[0], fs.OpenMode.READ_WRITE | fs.OpenMode.TRUNC);
  try {
    fs.writeSync(dst.fd, bytes.buffer);
  } finally {
    fs.closeSync(dst);
  }
  return true;
}

