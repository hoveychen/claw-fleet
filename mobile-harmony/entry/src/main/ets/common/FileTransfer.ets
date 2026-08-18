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
import { t } from './I18n';
import { RelayClient } from './Relay';

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

/// Open the system document picker, upload each chosen file through the relay's
/// `upload_attachment` (bytes → the desktop's user-attachments store), and return
/// the persistent paths. Oversize files are skipped with a toast; a cancelled
/// picker just yields an empty array.
export async function pickAndUploadAttachments(
  client: RelayClient,
  context: common.UIAbilityContext,
): Promise<Attachment[]> {
  const opts = new picker.DocumentSelectOptions();
  opts.maxSelectNumber = 9;
  const vp = new picker.DocumentViewPicker(context);
  const uris = await vp.select(opts);
  const out: Attachment[] = [];
  const b64 = new util.Base64Helper();
  for (const uri of uris) {
    const file = fs.openSync(uri, fs.OpenMode.READ_ONLY);
    try {
      const stat = fs.statSync(file.fd);
      if (stat.size > MAX_UPLOAD_BYTES) {
        promptAction.showToast({ message: t('「{0}」超过 10 MB 上限，已跳过', baseName(uri)), duration: 2500 });
        continue;
      }
      const ab = new ArrayBuffer(stat.size);
      fs.readSync(file.fd, ab);
      const name = baseName(uri);
      const params: UploadParams = { name, base64: b64.encodeToStringSync(new Uint8Array(ab)) };
      const resp = await client.request<UploadResp>('upload_attachment', params);
      out.push({ name, path: resp.path });
    } finally {
      fs.closeSync(file);
    }
  }
  return out;
}

/// Append uploaded attachment paths to the prompt as a `Context files:` block —
/// mirrors mobile-web withContextFiles; the desktop resolves these paths.
export function withContextFiles(prompt: string, attachments: Attachment[]): string {
  if (attachments.length === 0) {
    return prompt;
  }
  const lines = attachments.map((a: Attachment) => `- ${a.path}`).join('\n');
  return `${prompt}\n\nContext files:\n${lines}`;
}

/// Export a wiki-doc version: fetch its bytes via `wiki_export`, let the user pick
/// a save location (DocumentViewPicker.save), and write the bytes there. Returns
/// true when saved, false when the user cancelled the save dialog.
export async function exportWikiDoc(
  client: RelayClient,
  slug: string,
  version: string,
  context: common.UIAbilityContext,
): Promise<boolean> {
  const params: WikiExportParams = { slug, version };
  const payload = await client.request<WikiExportResp>('wiki_export', params);
  const opts = new picker.DocumentSaveOptions();
  opts.newFileNames = [payload.filename];
  const vp = new picker.DocumentViewPicker(context);
  const uris = await vp.save(opts);
  if (uris.length === 0) {
    return false;
  }
  const bytes = new util.Base64Helper().decodeSync(payload.base64);
  const dst = fs.openSync(uris[0], fs.OpenMode.READ_WRITE | fs.OpenMode.TRUNC);
  try {
    fs.writeSync(dst.fd, bytes.buffer);
  } finally {
    fs.closeSync(dst);
  }
  return true;
}
