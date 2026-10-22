import { beforeAll, describe, expect, it, vi } from "vitest";
import { setLang } from "./i18n";
import { classifyLibraryError } from "../../shared-ts/libraryErrors";
import { libraryErrorLine, libraryErrorOf, libraryErrorView } from "./libraryErrors";
import { TransportError } from "./transport";

describe("classifyLibraryError", () => {
  it.each([
    ["folder 'Invoices' already exists", "folder-exists", ["Invoices"]],
    ["folder 'a/b' not found", "folder-not-found", ["a/b"]],
    ["folder 'x' is not in this project's schema", "folder-not-found", ["x"]],
    ["'发票' still holds 3 item(s) — move them out first", "folder-not-empty", ["发票", "3"]],
    ["a folder needs a name", "folder-name-empty", []],
    ["'..' is not a usable folder name", "folder-name-invalid", [".."]],
    ["a folder name cannot contain control characters", "folder-name-invalid", []],
    ["folder path is more than 6 levels deep", "folder-too-deep", ["6"]],
    ["'公示' would share its document folder 'p/gongshi' with '公式' — pick a different name", "folder-name-clash", ["公示", "公式"]],
    ["artifact 'abc' not found", "item-not-found", ["abc"]],
    ["this upload is still being filed — undo it once it finishes", "upload-still-filing", []],
    ["unsupported file type .xyz", "unsupported-file", ["unsupported file type .xyz", "xyz"]],
    ["the AI could not read it (claude timed out after 90s); left unfiled", "ai-unreadable", []],
    ["over the 500-file limit for one upload", "upload-too-many", ["500"]],
    ["Failed to fetch", "offline", []],
    ["HTTP 502", "offline", []],
    ["sqlite open: disk I/O error", "unknown", []],
  ])("%s → %s", (raw, code, args) => {
    const got = classifyLibraryError(new Error(raw));
    expect(got.code).toBe(code);
    expect(got.args).toEqual(args);
    expect(got.raw).toBe(raw);
  });

  it("treats a transport failure as offline whatever its text", () => {
    expect(classifyLibraryError(new Error("folder 'x' already exists"), true).code).toBe("offline");
  });
});

describe("phone copy", () => {
  beforeAll(() => {
    // setLang stamps <html lang>; the node environment has no document.
    vi.stubGlobal("document", { documentElement: {} });
    setLang("zh");
  });

  it("renders a known error in Chinese with no raw detail", () => {
    const v = libraryErrorOf(new TransportError("folder 'Invoices' already exists", true));
    expect(v).toEqual({ message: "已经有叫「Invoices」的目录了，换个名字吧。" });
  });

  it("renders an unknown error generically, keeping the original as detail", () => {
    const v = libraryErrorOf(new TransportError("sqlite open: disk I/O error", true), "新建目录");
    expect(v.message).toBe("新建目录没成功，请稍后再试。");
    expect(v.detail).toBe("sqlite open: disk I/O error");
  });

  it("reads a dropped request as offline and keeps the transport's text as detail", () => {
    const v = libraryErrorOf(new TransportError("请求超时（桌面端可能离线）", false));
    expect(v.message).toBe("连不上桌面端，确认桌面端在线后再试。");
    expect(v.detail).toBe("请求超时（桌面端可能离线）");
  });

  it("gives every code but unknown its own copy", () => {
    const codes = [
      "offline", "folder-exists", "folder-not-found", "folder-not-empty", "folder-name-empty",
      "folder-name-invalid", "folder-name-too-long", "folder-too-deep", "folder-path-too-long",
      "folder-name-clash", "folder-into-itself", "project-root", "item-not-found", "item-name-taken",
      "upload-still-filing", "upload-not-filed", "upload-started", "upload-not-found", "upload-too-large",
      "upload-empty", "upload-no-project", "unsupported-file", "ai-unreadable", "draft-failed",
      "archive-kept", "skipped-link", "archive-too-deep", "upload-too-many", "archive-unsafe-path",
      "archive-damaged", "pdf-no-text", "pdf-encrypted", "new-folder-cap",
    ] as const;
    for (const code of codes) {
      const v = libraryErrorView({ code, args: ["a", "1"], raw: "x" });
      expect(v.message, code).not.toMatch(/没成功/);
      expect(v.message, code).toMatch(/[一-鿿]/);
    }
  });

  it("one-line form keeps an unrecognised note readable", () => {
    expect(libraryErrorLine("pdf has little or no text layer")).toBe("PDF 几乎没有可读的文字层。");
    expect(libraryErrorLine("weird thing")).toBe("没能处理：weird thing");
  });

  it("has English copy too", () => {
    setLang("en");
    try {
      expect(libraryErrorOf(new TransportError("a folder needs a name", true)).message).toBe("A folder needs a name.");
    } finally {
      setLang("zh");
    }
  });
});
