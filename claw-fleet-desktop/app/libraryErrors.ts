// 资料 errors in the desktop's language. Core answers in English; the shared
// classifier (shared-ts/libraryErrors.ts) recognises its messages and this maps
// each code to a `library.err.*` locale key. An unrecognised message gets the
// generic line with the original underneath, for whoever has to debug it.

import type { TFunction } from "i18next";
import {
  classifyLibraryError,
  libraryErrorNeedsDetail,
  type LibraryErrorCode,
} from "../../shared-ts/libraryErrors";

/** Copy per code, each a literal `t()` so the locale key gate sees it. */
function copyOf(t: TFunction, code: Exclude<LibraryErrorCode, "unknown">, a: string[]): string {
  switch (code) {
    case "offline":
      return t("library.err.offline", "连不上 Fleet，确认它还在运行后再试。");
    case "folder-exists":
      return t("library.err.folder_exists", "已经有叫「{{a0}}」的目录了，换个名字吧。", { a0: a[0] ?? "" });
    case "folder-not-found":
      return t("library.err.folder_not_found", "目录「{{a0}}」已经不在了，可能刚被改名或删除。", { a0: a[0] ?? "" });
    case "folder-not-empty":
      return t("library.err.folder_not_empty", "「{{a0}}」里还有 {{a1}} 份资料，先挪走再删。", { a0: a[0] ?? "", a1: a[1] ?? "" });
    case "folder-name-empty":
      return t("library.err.folder_name_empty", "目录名不能为空。");
    case "folder-name-invalid":
      return t("library.err.folder_name_invalid", "目录名不能是「.」或「..」，也不能含控制字符。");
    case "folder-name-too-long":
      return t("library.err.folder_name_too_long", "目录名太长了，每一层最多 {{a1}} 个字。", { a0: a[0] ?? "", a1: a[1] ?? "" });
    case "folder-too-deep":
      return t("library.err.folder_too_deep", "目录层级太深了，最多 {{a0}} 层。", { a0: a[0] ?? "" });
    case "folder-path-too-long":
      return t("library.err.folder_path_too_long", "目录路径太长了，换个短点的名字。");
    case "folder-name-clash":
      return t("library.err.folder_name_clash", "「{{a0}}」和已有目录「{{a1}}」会存到同一个位置，换个名字吧。", { a0: a[0] ?? "", a1: a[1] ?? "" });
    case "folder-into-itself":
      return t("library.err.folder_into_itself", "不能把目录挪进它自己里面。");
    case "project-root":
      return t("library.err.project_root", "项目根目录不能改名或删除。");
    case "item-not-found":
      return t("library.err.item_not_found", "这份资料已经不在了，可能刚被移走或删除。");
    case "item-name-taken":
      return t("library.err.item_name_taken", "目标目录里已有同名文档，换个目录试试。");
    case "upload-still-filing":
      return t("library.err.upload_still_filing", "这批上传还在整理中，等整理完再操作。");
    case "upload-not-filed":
      return t("library.err.upload_not_filed", "只有已归档的文件才能撤销或挪动。");
    case "upload-started":
      return t("library.err.upload_started", "这批上传已经开始整理了。");
    case "upload-not-found":
      return t("library.err.upload_not_found", "找不到这批上传，可能已被清除。");
    case "upload-too-large":
      return t("library.err.upload_too_large", "文件太大，超过了单个文件 512 MB 的上限。");
    case "upload-empty":
      return t("library.err.upload_empty", "没有可上传的文件。");
    case "upload-no-project":
      return t("library.err.upload_no_project", "先选一个要上传到的项目。");
    case "unsupported-file":
      return t("library.err.unsupported_file", "不支持这种文件格式（加密或旧版 Office 文件也读不了）。");
    case "ai-unreadable":
      return t("library.err.ai_unreadable", "AI 没能读懂这个文件，已留在未归档。");
    case "draft-failed":
      return t("library.err.draft_failed", "AI 没能起草目录结构，文件都留在了未归档。");
    case "archive-kept":
      return t("library.err.archive_kept", "{{a0}} 压缩包按原样存下了，Fleet 不拆这种格式；解压后再上传就能逐个归档。", { a0: a[0] ?? "" });
    case "skipped-link":
      return t("library.err.skipped_link", "跳过了链接或特殊文件。");
    case "archive-too-deep":
      return t("library.err.archive_too_deep", "文件夹嵌套太深，跳过了。");
    case "upload-too-many":
      return t("library.err.upload_too_many", "一次最多上传 {{a0}} 个文件，多出的没收。", { a0: a[0] ?? "" });
    case "archive-unsafe-path":
      return t("library.err.archive_unsafe_path", "压缩包里的路径不安全，跳过了。");
    case "archive-damaged":
      return t("library.err.archive_damaged", "压缩包读不了，可能已损坏。");
    case "pdf-no-text":
      return t("library.err.pdf_no_text", "PDF 几乎没有可读的文字层。");
    case "pdf-encrypted":
      return t("library.err.pdf_encrypted", "PDF 加了密，读不了内容。");
    case "new-folder-cap":
      return t("library.err.new_folder_cap", "AI 想新建目录「{{a0}}」，但这批上传已经新建了 {{a1}} 个目录，先留在未归档。", { a0: a[0] ?? "", a1: a[1] ?? "" });
  }
}

/** A thrown 资料 error as one localized string; a generic line keeps the
 *  original message on a second line (the banner is `white-space: pre-line`). */
export function libraryErrorText(t: TFunction, e: unknown): string {
  const err = classifyLibraryError(e);
  const message =
    err.code === "unknown"
      ? t("library.err.unknown", "操作没成功，请稍后再试。")
      : copyOf(t, err.code, err.args);
  return libraryErrorNeedsDetail(err) ? `${message}\n${err.raw}` : message;
}
