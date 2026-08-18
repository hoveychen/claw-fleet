/// On-device file transfer for the 元服务 client (feature-gaps P5) — attachment
/// selection for the composer and wiki-doc export. Both hang off
/// DocumentViewPicker, whose `select` and `save` are `@atomicservice` (verified
/// against @ohos.file.picker.d.ts, @since 12), so they work inside the atomic-
/// service sandbox. There is NO outbound ShareKit / systemShare in this SDK
/// (only ShareExtensionAbility for *receiving* shares), so the "分享" half of P5
/// degrades to the platform save dialog — export writes the file to a user-chosen
/// location instead of raising a share sheet.
///
/// ArkTS shape notes: relay params / picker options are declared interfaces or
/// SDK classes (never Record literals); base64 goes through util.Base64Helper;
/// bytes move via the sync fileIo API (files are capped at 10 MiB).

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



/// Export a wiki-doc version: fetch its bytes via `wiki_export`, let the user pick
/// a save location (DocumentViewPicker.save), and write the bytes there. Returns
/// true when saved, false when the user cancelled the save dialog.
/// 把一段 base64 存成用户选定位置的文件。
///
/// 从 exportWikiDoc 里抽出来的后半段：WebShell 下 relay 连接归 web 层，原生
/// 拿不到 RelayClient，但「让用户挑个位置把字节写下去」这件事仍然只有原生
/// 能做（WebView 里 <a download> 不会触发系统下载）。web 侧通过
/// javaScriptProxy 把文件名和 base64 递过来调这里。
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

