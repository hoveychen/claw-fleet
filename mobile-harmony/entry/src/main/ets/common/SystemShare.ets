import { common } from '@kit.AbilityKit';
import { fileIo as fs, fileUri } from '@kit.CoreFileKit';
import { util } from '@kit.ArkTS';
import { systemShare } from '@kit.ShareKit';

/// 系统分享 —— `navigator.share` 在 ArkWeb 里的原生一端。
///
/// mobile-web 的 WikiDocView 导出时本就优先走 Web Share（`navigator.share({files})`），
/// 拿不到才退回 `<a download>`。所以这里不需要新功能，只要把缺失的那半边补上，
/// web 侧一行不用改。
///
/// 元服务时代这条路是堵死的：@hms.collaboration.systemShare 整个文件没有一处
/// `@atomicservice` 标记，所以 feature-gaps P5 只能降级成「保存到选定位置」。
/// 转普通应用后限制解除。
///
/// 分享要的是 uri 而不是字节，所以先把内容落进 cacheDir —— 缓存目录会被系统
/// 回收，不会长期占用户空间。
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
    // 有具体 mime 就报具体的，让接收方（微信、备忘录…）能正确取舍；
    // 拿不到就退到通用文件类型，总比分享不出去强。
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
