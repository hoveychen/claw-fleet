import { common } from '@kit.AbilityKit';
import { fileIo as fs, fileUri } from '@kit.CoreFileKit';
import { util } from '@kit.ArkTS';
import { systemShare } from '@kit.ShareKit';

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
    // If we have a specific mime type, report it to let the receiver (WeChat, Notes, etc.) make
    // the right choice; if we don't have it, fall back to a generic file type — anything is better than not sharing.
    utd: mimeType.length > 0 ? mimeType : 'general.file',
    uri: fileUri.getUriFromPath(path),
    title: safeName,
  };
  const data = new systemShare.SharedData(record);
  const controller = new systemShare.ShareController(data);
  await controller.show(context, {
    selectionMode: systemShare.SelectionMode.SINGLE,
  });
}
