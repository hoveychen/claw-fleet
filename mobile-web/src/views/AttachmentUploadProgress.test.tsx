// 手机上挑一张图到 chip 出现之间隔着一次 relay 往返（不是本机拷贝），那几秒里
// 附件条原来是空的，只有「+」变成一个小转圈。这两组测试钉住补上的反馈：占位 chip
// 在同一次点击里就出现，且一个文件失败不再把后面的文件一起带走。
import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { AttachmentThumbs } from "./AttachmentThumb";
import { uploadAttachmentFile } from "./Composer";
import type { FleetTransport } from "../transport";

vi.mock("@capacitor/core", () => ({ Capacitor: { isNativePlatform: () => false } }));

describe("上传中的占位 chip", () => {
  it("只有 pending、还没有任何已落地路径时也要渲染", () => {
    const html = renderToStaticMarkup(
      <AttachmentThumbs
        paths={[]}
        pending={[{ id: "u1", name: "shot.png", previewUrl: "blob:local" }]}
        client={null}
      />,
    );
    expect(html).toContain('aria-busy="true"');
    // 本机 blob 是免费的，所以缩略图先于字节出现。
    expect(html).toContain("blob:local");
  });

  it("非图片走文件名 chip，同样标 busy", () => {
    const html = renderToStaticMarkup(
      <AttachmentThumbs paths={[]} pending={[{ id: "u2", name: "report.pdf" }]} client={null} />,
    );
    expect(html).toContain('aria-busy="true"');
    expect(html).toContain("report.pdf");
  });

  it("没有 pending 时行为不变：空列表不占高度", () => {
    expect(renderToStaticMarkup(<AttachmentThumbs paths={[]} client={null} />)).toBe("");
  });
});

describe("逐文件上传", () => {
  const client = {
    request: vi.fn(async () => ({ path: "/home/u/.fleet/user-attachments/k/shot.png" })),
  } as unknown as FleetTransport;

  function file(name: string, size: number, type = "image/png"): File {
    const f = new File([new Uint8Array([1])], name, { type });
    Object.defineProperty(f, "size", { value: size });
    return f;
  }

  // mobile-web 不装 jsdom，这两样是浏览器给的，测试里自己顶上。
  const alert = vi.fn();
  vi.stubGlobal("window", { alert });
  vi.stubGlobal(
    "FileReader",
    class {
      result = "data:image/png;base64,AQ==";
      onload: (() => void) | null = null;
      onerror: (() => void) | null = null;
      readAsDataURL() {
        queueMicrotask(() => this.onload?.());
      }
    },
  );

  it("超限文件返回 null 而不是抛错——它不该中断这一批里的其他文件", async () => {
    alert.mockClear();
    const out = await uploadAttachmentFile(client, file("huge.png", 20 * 1024 * 1024));
    expect(out).toBeNull();
    expect(alert).toHaveBeenCalled();
  });

  it("沿用调用方已经做好的本机预览，不再多造一个 blob", async () => {
    const out = await uploadAttachmentFile(client, file("shot.png", 1024), "blob:already");
    expect(out?.previewUrl).toBe("blob:already");
    expect(out?.path).toContain("user-attachments");
  });
});
