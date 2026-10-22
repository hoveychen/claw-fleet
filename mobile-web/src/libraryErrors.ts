// 资料 errors in the phone's language: core answers in English (see
// shared-ts/libraryErrors.ts for why and for the recognised messages); this is
// the phone's copy for each code. A transport failure (no verdict from the
// host) reads as "offline" whatever its text; an unrecognised message gets a
// generic line with the original kept as `detail`.

import { t } from "./i18n";
import { TransportError } from "./transport";
import {
  classifyLibraryError,
  libraryErrorNeedsDetail,
  type LibraryError,
  type LibraryErrorCode,
} from "../../shared-ts/libraryErrors";

export { classifyLibraryError, type LibraryError } from "../../shared-ts/libraryErrors";

/** Copy per code, each a literal `t()` so the i18n key gate sees it; `args`
 *  are the classified names. `unknown` is handled by the caller. */
function copyOf(code: Exclude<LibraryErrorCode, "unknown">, a: string[]): string {
  switch (code) {
    case "offline":
      return t("连不上桌面端，确认桌面端在线后再试。");
    case "folder-exists":
      return t("已经有叫「{0}」的目录了，换个名字吧。", a[0] ?? "");
    case "folder-not-found":
      return t("目录「{0}」已经不在了，可能刚被改名或删除。", a[0] ?? "");
    case "folder-not-empty":
      return t("「{0}」里还有 {1} 份资料，先挪走再删。", a[0] ?? "", a[1] ?? "");
    case "folder-name-empty":
      return t("目录名不能为空。");
    case "folder-name-invalid":
      return t("目录名不能是「.」或「..」，也不能含控制字符。");
    case "folder-name-too-long":
      return t("目录名太长了，每一层最多 {1} 个字。", a[0] ?? "", a[1] ?? "");
    case "folder-too-deep":
      return t("目录层级太深了，最多 {0} 层。", a[0] ?? "");
    case "folder-path-too-long":
      return t("目录路径太长了，换个短点的名字。");
    case "folder-name-clash":
      return t("「{0}」和已有目录「{1}」会存到同一个位置，换个名字吧。", a[0] ?? "", a[1] ?? "");
    case "folder-into-itself":
      return t("不能把目录挪进它自己里面。");
    case "project-root":
      return t("项目根目录不能改名或删除。");
    case "item-not-found":
      return t("这份资料已经不在了，可能刚被移走或删除。");
    case "item-name-taken":
      return t("目标目录里已有同名文档，换个目录试试。");
    case "upload-still-filing":
      return t("这批上传还在整理中，等整理完再操作。");
    case "upload-not-filed":
      return t("只有已归档的文件才能撤销或挪动。");
    case "upload-started":
      return t("这批上传已经开始整理了。");
    case "upload-not-found":
      return t("找不到这批上传，可能已被清除。");
    case "upload-too-large":
      return t("文件太大，超过了单个文件 512 MB 的上限。");
    case "upload-empty":
      return t("没有可上传的文件。");
    case "upload-no-project":
      return t("先选一个要上传到的项目。");
    case "unsupported-file":
      return t("不支持这种文件格式（加密或旧版 Office 文件也读不了）。");
    case "ai-unreadable":
      return t("AI 没能读懂这个文件，已留在未归档。");
    case "draft-failed":
      return t("AI 没能起草目录结构，文件都留在了未归档。");
    case "archive-kept":
      return t("{0} 压缩包按原样存下了，Fleet 不拆这种格式；解压后再上传就能逐个归档。", a[0] ?? "");
    case "skipped-link":
      return t("跳过了链接或特殊文件。");
    case "archive-too-deep":
      return t("文件夹嵌套太深，跳过了。");
    case "upload-too-many":
      return t("一次最多上传 {0} 个文件，多出的没收。", a[0] ?? "");
    case "archive-unsafe-path":
      return t("压缩包里的路径不安全，跳过了。");
    case "archive-damaged":
      return t("压缩包读不了，可能已损坏。");
    case "pdf-no-text":
      return t("PDF 几乎没有可读的文字层。");
    case "pdf-encrypted":
      return t("PDF 加了密，读不了内容。");
    case "new-folder-cap":
      return t("AI 想新建目录「{0}」，但这批上传已经新建了 {1} 个目录，先留在未归档。", a[0] ?? "", a[1] ?? "");
  }
}

export interface LibraryErrorView {
  /** The localized line. */
  message: string;
  /** The original text, for a secondary line — only when the line is generic. */
  detail?: string;
}

/** Render a classified error. `action` names what failed ("新建目录") for
 *  the generic line. */
export function libraryErrorView(err: LibraryError, action?: string): LibraryErrorView {
  const message =
    err.code === "unknown"
      ? action
        ? t("{0}没成功，请稍后再试。", action)
        : t("操作没成功，请稍后再试。")
      : copyOf(err.code, err.args);
  return libraryErrorNeedsDetail(err) && err.raw !== message ? { message, detail: err.raw } : { message };
}

/** Classify and render a thrown error in one go. */
export function libraryErrorOf(e: unknown, action?: string): LibraryErrorView {
  const transportFailed = e instanceof TransportError && !e.remote;
  return libraryErrorView(classifyLibraryError(e, transportFailed), action);
}

/** One-line form, for places that only hold a string (a batch item's note). */
export function libraryErrorLine(raw: string): string {
  const err = classifyLibraryError(raw);
  if (err.code === "unknown") return t("没能处理：{0}", err.raw);
  return libraryErrorView(err).message;
}
