import { describe, expect, it } from "vitest";
import { shareToPrompt, type IncomingShare } from "./shareTarget";

const empty: IncomingShare = { title: "", texts: [], files: [] };

describe("shareToPrompt", () => {
  it("uses the shared text", () => {
    expect(shareToPrompt({ ...empty, texts: ["看看这个 bug"] })).toBe("看看这个 bug");
  });

  it("joins multiple shared texts", () => {
    expect(shareToPrompt({ ...empty, texts: ["第一段", "第二段"] })).toBe("第一段\n\n第二段");
  });

  // Android senders routinely set title and text to the same string (sharing a
  // bare URL does exactly this); echoing it twice looks like a bug.
  it("drops a title that just repeats the text", () => {
    const url = "https://example.com/a";
    expect(shareToPrompt({ ...empty, title: url, texts: [url] })).toBe(url);
  });

  it("keeps a title that adds information", () => {
    expect(shareToPrompt({ ...empty, title: "文章标题", texts: ["https://x.dev"] })).toBe(
      "文章标题\n\nhttps://x.dev",
    );
  });

  it("names shared files so the prompt says what came along", () => {
    const share: IncomingShare = {
      ...empty,
      texts: ["帮我看下"],
      files: [
        { uri: "content://a", name: "crash.log", mimeType: "text/plain" },
        { uri: "content://b", name: "shot.png", mimeType: "image/png" },
      ],
    };
    expect(shareToPrompt(share)).toBe("帮我看下\n\n[共享文件] crash.log, shot.png");
  });

  it("ignores blank and whitespace-only text", () => {
    expect(shareToPrompt({ ...empty, texts: ["", "   "] })).toBe("");
    expect(shareToPrompt(empty)).toBe("");
  });

  it("trims surrounding whitespace", () => {
    expect(shareToPrompt({ ...empty, texts: ["  内容  "] })).toBe("内容");
  });
});
