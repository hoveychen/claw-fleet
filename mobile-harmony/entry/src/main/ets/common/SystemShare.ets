import { common } from '@kit.AbilityKit';
import { fileIo as fs, fileUri } from '@kit.CoreFileKit';
import { util } from '@kit.ArkTS';
import { systemShare } from '@kit.ShareKit';
import { uniformTypeDescriptor as utd } from '@kit.ArkData';

/// ShareKit's `utd` takes a Uniform Type Descriptor (`general.markdown`), not a MIME type.
/// Handing it a raw MIME string matched no native HarmonyOS share target, so the sheet only
/// listed Android apps running under the compat layer (those accept anything). Resolve via
/// MIME, then the filename extension; unknown lookups come back as dynamic `flex.*` types,
/// which match nothing either, so those fall through to `general.file`.
function resolveUtd(filename: string, mimeType: string): string {
  try {
    if (mimeType.length > 0) {
      const byMime = utd.getUniformDataTypeByMIMEType(mimeType);
      if (!byMime.startsWith('flex.')) {
        return byMime;
      }
    }
    const dot = filename.lastIndexOf('.');
    if (dot >= 0) {
      const byExt = utd.getUniformDataTypeByFilenameExtension(filename.substring(dot));
      if (!byExt.startsWith('flex.')) {
        return byExt;
      }
    }
  } catch (e) {
    // Fall through to the generic type.
  }
  return utd.UniformDataType.FILE;
}

/// System share — native end of `navigator.share` in ArkWeb.
///
/// mobile-web's WikiDocView export already prioritizes Web Share (`navigator.share({files})`),
/// falling back to `<a download>` if unavailable. No new functionality needed, just fill the
/// missing half, web side requires no changes.
///
/// This path was blocked in the atomic service era: @hms.collaboration.systemShare has no
/// `@atomicservice` markers anywhere, so feature-gaps P5 downgraded to "save to chosen location".
/// After switching to regular app, the restriction lifts.
///
/// Share needs uri not bytes, so write content to cacheDir first — the system will reclaim
/// cache space, not permanently consuming user storage.
export async function shareFile(
  context: common.UIAbilityContext,
  filename: string,
  mimeType: string,
  base64: string,
): Promise<void> {
  const safeName = filename.length > 0 ? filename : 'share';
  const path = `${context.cacheDir}/${safeName}`;
  const bytes = new util.Base64Helper().decodeSync(base64);
  const fd = fs.openSync(path, fs.OpenMode.CREATE | fs.OpenMode.READ_WRITE | fs.OpenMode.TRUNC);
  try {
    fs.writeSync(fd.fd, bytes.buffer);
  } finally {
    fs.closeSync(fd);
  }

  const record: systemShare.SharedRecord = {
    // A specific type lets receivers (WeChat, Notes, etc.) offer themselves; the generic file
    // type is the fallback — anything is better than not sharing.
    utd: resolveUtd(safeName, mimeType),
    uri: fileUri.getUriFromPath(path),
    title: safeName,
  };
  const data = new systemShare.SharedData(record);
  const controller = new systemShare.ShareController(data);
  await controller.show(context, {
    selectionMode: systemShare.SelectionMode.SINGLE,
  });
}
