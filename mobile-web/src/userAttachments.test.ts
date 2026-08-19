import { describe, expect, it } from "vitest";
import {
  attachmentName,
  attachmentRef,
  fetchAttachmentImage,
  isRenderableImage,
  splitContextFiles,
} from "./userAttachments";
import type { RelayClient } from "./relay";

const DEFAULT_CONTROL_TIMEOUT_MS = 15_000;

describe("attachmentRef —— 路径 → store 坐标", () => {
  it("认出 store 里的附件", () => {
    expect(attachmentRef("/Users/x/.fleet/user-attachments/ab12cd34/shot.png")).toEqual({
      key: "ab12cd34",
      name: "shot.png",
    });
  });

  it("认出 Windows 反斜杠路径（桌面端在 Windows 上冻进 transcript 的形状）", () => {
    expect(attachmentRef("C:\\Users\\x\\.fleet\\user-attachments\\ab12\\a.png")).toEqual({
      key: "ab12",
      name: "a.png",
    });
  });

  it("认出 store 之前的 $TMPDIR/fleet-pasted 旧路径", () => {
    expect(attachmentRef("/var/folders/t/fleet-pasted/pasted-1.png")).toEqual({
      key: "_pasted",
      name: "pasted-1.png",
    });
  });

  it("用户自己挑的普通路径不给坐标（不许拿它去读桌面磁盘）", () => {
    expect(attachmentRef("/Users/x/Desktop/secret.png")).toBeNull();
    expect(attachmentRef("/etc/passwd")).toBeNull();
  });
});

describe("splitContextFiles —— 剥掉 composer 拼的尾巴", () => {
  it("正文与路径分离", () => {
    const text = "看下这个\n\nContext files:\n- /a/one.png\n- /b/two.pdf";
    expect(splitContextFiles(text)).toEqual({
      body: "看下这个",
      paths: ["/a/one.png", "/b/two.pdf"],
    });
  });

  it("正文里手打的 “Context files:” 不动它", () => {
    const text = "Context files: 你确定吗？\n- 这不是路径";
    expect(splitContextFiles(text).paths).toEqual([]);
    expect(splitContextFiles(text).body).toBe(text);
  });

  it("尾块后面还有正文时不匹配（只认结尾）", () => {
    const text = "a\n\nContext files:\n- /a/one.png\n\n然后我又打了字";
    expect(splitContextFiles(text).paths).toEqual([]);
  });
});

describe("isRenderableImage / attachmentName", () => {
  it("按扩展名判断能否内联渲染", () => {
    expect(isRenderableImage("a.PNG")).toBe(true);
    expect(isRenderableImage("a.jpeg")).toBe(true);
    expect(isRenderableImage("a.pdf")).toBe(false);
    expect(isRenderableImage("a")).toBe(false);
  });

  it("取文件名（正反斜杠都认）", () => {
    expect(attachmentName("/a/b/c.png")).toBe("c.png");
    expect(attachmentName("C:\\a\\b\\c.png")).toBe("c.png");
  });
});

describe("fetchAttachmentImage", () => {
  function captor() {
    const calls: Array<{ method: string; params?: unknown; timeoutMs?: number }> = [];
    const client = {
      request: (method: string, params?: unknown, timeoutMs?: number) => {
        calls.push({ method, params, timeoutMs });
        return Promise.resolve({ mime: "image/jpeg", base64: "" });
      },
    } as unknown as RelayClient;
    return { client, calls };
  }

  it("默认取缩略图，full 时显式请求原图", async () => {
    const { client, calls } = captor();
    await fetchAttachmentImage(client, { key: "k", name: "a.png" });
    await fetchAttachmentImage(client, { key: "k", name: "a.png" }, true);
    expect(calls[0].params).toEqual({ key: "k", name: "a.png", full: false });
    expect(calls[1].params).toEqual({ key: "k", name: "a.png", full: true });
  });

  // 与 decision_asset 同源的坑：MB 级图片走 15s 控制默认超时会在慢网下静默早退，
  // 迟到的 reply 被丢弃，<img> 永远卡在 loading。
  it("超时远大于 15s 控制消息默认值", async () => {
    const { client, calls } = captor();
    await fetchAttachmentImage(client, { key: "k", name: "a.png" }, true);
    expect(calls[0].method).toBe("user_attachment");
    expect(calls[0].timeoutMs ?? 0).toBeGreaterThan(DEFAULT_CONTROL_TIMEOUT_MS);
  });
});
