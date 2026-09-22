import { describe, expect, it } from "vitest";
import {
  attachmentName,
  attachmentRef,
  fetchAttachmentImage,
  imageAttachmentRef,
  isRenderableImage,
  splitAnswerAttachments,
  splitContextFiles,
} from "./userAttachments";
import type { RelayClient } from "./relay";

const DEFAULT_CONTROL_TIMEOUT_MS = 15_000;

describe("attachmentRef — path to store coordinates", () => {
  it("recognizes attachment in store", () => {
    expect(attachmentRef("/Users/x/.fleet/user-attachments/ab12cd34/shot.png")).toEqual({
      key: "ab12cd34",
      name: "shot.png",
    });
  });

  it("recognizes Windows backslash paths (as stored in transcript on Windows desktop)", () => {
    expect(attachmentRef("C:\\Users\\x\\.fleet\\user-attachments\\ab12\\a.png")).toEqual({
      key: "ab12",
      name: "a.png",
    });
  });

  it("recognizes legacy $TMPDIR/fleet-pasted paths from before store", () => {
    expect(attachmentRef("/var/folders/t/fleet-pasted/pasted-1.png")).toEqual({
      key: "_pasted",
      name: "pasted-1.png",
    });
  });

  it("user-chosen regular paths return null (prevent desktop disk access)", () => {
    expect(attachmentRef("/Users/x/Desktop/secret.png")).toBeNull();
    expect(attachmentRef("/etc/passwd")).toBeNull();
  });
});

describe("splitContextFiles — strip composer-appended tail", () => {
  it("separates body and paths", () => {
    const text = "看下这个\n\nContext files:\n- /a/one.png\n- /b/two.pdf";
    expect(splitContextFiles(text)).toEqual({
      body: "看下这个",
      paths: ["/a/one.png", "/b/two.pdf"],
    });
  });

  it('preserves manually typed "Context files:" in body', () => {
    const text = "Context files: 你确定吗？\n- 这不是路径";
    expect(splitContextFiles(text).paths).toEqual([]);
    expect(splitContextFiles(text).body).toBe(text);
  });

  it("no match when text follows footer block (only end-of-string counts)", () => {
    const text = "a\n\nContext files:\n- /a/one.png\n\n然后我又打了字";
    expect(splitContextFiles(text).paths).toEqual([]);
  });
});

describe("splitAnswerAttachments — strip @path from decision answers", () => {
  it("separates option label and attachment paths", () => {
    expect(splitAnswerAttachments("好的 @/Users/x/.fleet/user-attachments/k/a.png")).toEqual({
      core: "好的",
      attachments: ["/Users/x/.fleet/user-attachments/k/a.png"],
    });
  });

  it("recognizes home directory paths starting with @~", () => {
    expect(splitAnswerAttachments("@~/shot.png").attachments).toEqual(["~/shot.png"]);
  });

  it("returns unchanged when no attachments", () => {
    expect(splitAnswerAttachments("方案 A")).toEqual({ core: "方案 A", attachments: [] });
  });

  it("does not treat @mention (non-path) in body as attachment", () => {
    expect(splitAnswerAttachments("问问 @someone").attachments).toEqual([]);
  });
});

describe("isRenderableImage / attachmentName", () => {
  it("determines inline renderability by extension", () => {
    expect(isRenderableImage("a.PNG")).toBe(true);
    expect(isRenderableImage("a.jpeg")).toBe(true);
    expect(isRenderableImage("a.pdf")).toBe(false);
    expect(isRenderableImage("a")).toBe(false);
  });

  it("extracts filename (both forward and backslash paths)", () => {
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

  it("fetches thumbnail by default, explicitly requests full image when full=true", async () => {
    const { client, calls } = captor();
    await fetchAttachmentImage(client, { key: "k", name: "a.png" });
    await fetchAttachmentImage(client, { key: "k", name: "a.png" }, true);
    expect(calls[0].params).toEqual({ key: "k", name: "a.png", full: false });
    expect(calls[1].params).toEqual({ key: "k", name: "a.png", full: true });
  });

  // Same pitfall as decision_asset: MB-sized images over 15s control default timeout
  // silently exit early on slow networks, late replies get discarded, <img> stays loading forever.
  it("timeout far exceeds 15s control message default", async () => {
    const { client, calls } = captor();
    await fetchAttachmentImage(client, { key: "k", name: "a.png" }, true);
    expect(calls[0].method).toBe("user_attachment");
    expect(calls[0].timeoutMs ?? 0).toBeGreaterThan(DEFAULT_CONTROL_TIMEOUT_MS);
  });
});

// A picked file never enters the store, so history holds only its host path.
describe("imageAttachmentRef", () => {
  it("prefers store coordinates when the path is in the store", () => {
    expect(imageAttachmentRef("/Users/x/.fleet/user-attachments/ab12/shot.png")).toEqual({
      key: "ab12",
      name: "shot.png",
    });
  });

  it("addresses a picked image by its host path", () => {
    expect(imageAttachmentRef("/Users/x/Pictures/wife_1.jpg")).toEqual({
      path: "/Users/x/Pictures/wife_1.jpg",
    });
    expect(imageAttachmentRef("C:\\Users\\x\\a.png")).toEqual({ path: "C:\\Users\\x\\a.png" });
  });

  it("refuses non-images and relative paths", () => {
    expect(imageAttachmentRef("/etc/passwd")).toBeNull();
    expect(imageAttachmentRef("/Users/x/spec.pdf")).toBeNull();
    expect(imageAttachmentRef("pics/a.png")).toBeNull();
  });

  it("sends only the path to the relay for a picked image", async () => {
    const calls: unknown[] = [];
    const client = {
      request: (_m: string, params?: unknown) => {
        calls.push(params);
        return Promise.resolve({ mime: "image/jpeg", base64: "" });
      },
    } as unknown as RelayClient;
    await fetchAttachmentImage(client, { path: "/Users/x/Pictures/wife_1.jpg" });
    expect(calls[0]).toEqual({ path: "/Users/x/Pictures/wife_1.jpg", full: false });
  });
});
