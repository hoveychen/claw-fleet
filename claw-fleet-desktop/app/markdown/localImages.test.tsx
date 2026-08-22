// @vitest-environment jsdom
// An agent that finishes a screenshot run writes its results as ordinary
// markdown image refs with absolute host paths:
//
//   ![开场](/Users/me/workspace/genie-mvp/shots/01-开场.png)
//
// Nothing in the markdown renderer used to claim `img`, so react-markdown
// emitted a bare <img src="/Users/…"> — a path the webview resolves against its
// own origin (`tauri://localhost`), which serves only the bundled frontend. The
// whole set came out as broken-image placeholders even though every file was on
// disk. Fleet deliberately has no asset protocol (the backend may be remote), so
// the bytes have to come back through the Backend — `read_external_file`, which
// already exists for the 文件 page and is implemented on both transports.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import ReactMarkdown from "react-markdown";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invoke(...a) }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

await import("../i18n");
const { TextBlock } = await import("../components/blocks/TextBlock");
const { safeMarkdownComponents } = await import("./safeLinks");

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const PNG_B64 = "iVBORw0KGgo=";

let container: HTMLDivElement | null = null;
let root: Root | null = null;

beforeEach(() => {
  invoke.mockReset();
});

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
});

/** Mount and flush the effect that fetches the bytes. */
async function mount(node: React.ReactElement) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  await act(async () => {
    root!.render(node);
  });
}

function imgSrc(): string | null {
  return container!.querySelector("img")?.getAttribute("src") ?? null;
}

describe("markdown 里的本地图片", () => {
  it("绝对本地路径经 read_external_file 取回字节，渲成 data URL", async () => {
    invoke.mockResolvedValue({ kind: "image", base64: PNG_B64, mime: "image/png", sizeBytes: 11 });

    await mount(<TextBlock text="![开场](/Users/me/shots/01-开场.png)" />);

    expect(invoke).toHaveBeenCalledWith("read_external_file", {
      path: "/Users/me/shots/01-开场.png",
    });
    expect(imgSrc()).toBe(`data:image/png;base64,${PNG_B64}`);
  });

  it("同样接管共享的 safeMarkdownComponents（决策卡 / 知识库 / 日报都用它）", async () => {
    invoke.mockResolvedValue({ kind: "image", base64: PNG_B64, mime: "image/png", sizeBytes: 11 });

    await mount(
      <ReactMarkdown components={safeMarkdownComponents}>
        {"![shot](/tmp/shot.png)"}
      </ReactMarkdown>,
    );

    expect(invoke).toHaveBeenCalledWith("read_external_file", { path: "/tmp/shot.png" });
    expect(imgSrc()).toBe(`data:image/png;base64,${PNG_B64}`);
  });

  it("http(s) 图片直通，不去读文件", async () => {
    await mount(<TextBlock text="![web](https://example.com/a.png)" />);

    expect(invoke).not.toHaveBeenCalled();
    expect(imgSrc()).toBe("https://example.com/a.png");
  });

  // Admitted by markdownUrlTransform (see markdown/plugins): it goes straight to
  // <img src>, so there is nothing to read off disk.
  it("data:image URL 直接内联，不去读文件", async () => {
    await mount(<TextBlock text={`![inline](data:image/png;base64,${PNG_B64})`} />);

    expect(invoke).not.toHaveBeenCalled();
    expect(imgSrc()).toBe(`data:image/png;base64,${PNG_B64}`);
  });

  it("file:// 也当本地路径读，而不是丢给 webview", async () => {
    invoke.mockResolvedValue({ kind: "image", base64: PNG_B64, mime: "image/png", sizeBytes: 11 });

    await mount(<TextBlock text="![f](file:///Users/me/shots/a%20b.png)" />);

    // Percent-decoded: the src side of a markdown image is a URL, so a space
    // arrives as %20 and the file on disk is named with a real space.
    expect(invoke).toHaveBeenCalledWith("read_external_file", {
      path: "/Users/me/shots/a b.png",
    });
  });

  it("javascript: 之类的 src 仍被拦掉", async () => {
    await mount(<TextBlock text="![x](javascript:alert(1))" />);

    expect(invoke).not.toHaveBeenCalled();
    expect(container!.querySelector("img")).toBeNull();
  });

  // The sanitize schema admits the whole `data:` scheme (it cannot express a
  // mime restriction); markdownUrlTransform is the half that keeps it to images.
  // Assert the composed pipeline, not just the transform in isolation.
  it("非图片的 data: 文档不会渲染", async () => {
    await mount(<TextBlock text="![x](data:text/html,<script>alert(1)</script>)" />);

    expect(container!.querySelector("img")).toBeNull();
  });

  it("读取失败时留下可见的降级提示，而不是空盒子", async () => {
    invoke.mockRejectedValue("external path: No such file or directory");

    await mount(<TextBlock text="![缺图](/Users/me/gone.png)" />);

    expect(container!.querySelector("[data-testid='markdown-image-failed']")).toBeTruthy();
    expect(container!.textContent).not.toBe("");
  });

  it("超过预览上限（后端回 Binary）也给可见提示", async () => {
    invoke.mockResolvedValue({ kind: "binary", sizeBytes: 20 * 1024 * 1024 });

    await mount(<TextBlock text="![巨图](/Users/me/huge.png)" />);

    expect(container!.querySelector("[data-testid='markdown-image-failed']")).toBeTruthy();
  });
});
