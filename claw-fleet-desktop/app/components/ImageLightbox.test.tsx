// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const { invoke, save, writeImage, imageNew, imageClose } = vi.hoisted(() => ({
  invoke: vi.fn(), save: vi.fn(), writeImage: vi.fn(), imageNew: vi.fn(), imageClose: vi.fn(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/image", () => ({ Image: { new: imageNew } }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ save }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeImage }));
vi.mock("react-i18next", () => ({ useTranslation: () => ({ t: (_key: string, fallback: string) => fallback }) }));
vi.mock("../hostEnv", () => ({ isWebBuild: () => false }));

import { ImageLightbox } from "./ImageLightbox";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
let root: Root;
let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
  vi.stubGlobal("fetch", vi.fn().mockResolvedValue({ ok: true, blob: async () => new Blob(["image bytes"], { type: "image/png" }) }));
  save.mockResolvedValue("/tmp/picture.png");
  invoke.mockResolvedValue(undefined);
  writeImage.mockResolvedValue(undefined);
  imageClose.mockResolvedValue(undefined);
  imageNew.mockResolvedValue({ close: imageClose });
});

afterEach(async () => {
  await act(async () => root.unmount());
  host.remove();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  vi.clearAllMocks();
});

it("copies the decoded image to the native clipboard", async () => {
  vi.stubGlobal("createImageBitmap", vi.fn().mockResolvedValue({ width: 1, height: 1, close: vi.fn() }));
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({
    drawImage: vi.fn(), getImageData: () => ({ data: new Uint8ClampedArray([1, 2, 3, 255]) }),
  } as unknown as CanvasRenderingContext2D);
  await act(async () => root.render(<ImageLightbox src="data:image/png;base64,AA==" onClose={() => {}} />));
  await act(async () => { document.querySelector<HTMLButtonElement>('button[aria-label="复制图片"]')!.click(); });
  await vi.waitFor(() => expect(writeImage).toHaveBeenCalled());
  expect(imageNew).toHaveBeenCalledWith(new Uint8Array([1, 2, 3, 255]), 1, 1);
  expect(imageClose).toHaveBeenCalled();
});

it("opens the macOS share menu with the original image bytes", async () => {
  vi.spyOn(navigator, "platform", "get").mockReturnValue("MacIntel");
  await act(async () => root.render(<ImageLightbox src="fleet-attachment://localhost/id/picture.png" onClose={() => {}} />));
  await act(async () => { document.querySelector<HTMLButtonElement>('button[aria-label="分享图片"]')!.click(); });
  await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("share_preview_image", { base64: btoa("image bytes") }));
});

it("zooms the message image, resets it, and saves the original bytes", async () => {
  await act(async () => root.render(<ImageLightbox src="fleet-attachment://localhost/id/picture.png" alt="picture.png" onClose={() => {}} />));
  const button = (label: string) => document.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)!;
  await act(async () => button("放大").click());
  expect(document.body.textContent).toContain("150%");
  await act(async () => button("适合窗口").click());
  expect(document.body.textContent).toContain("100%");
  await act(async () => { button("保存图片").click(); await new Promise((resolve) => setTimeout(resolve, 0)); });
  expect(save).toHaveBeenCalledWith({ defaultPath: "picture.png" });
  await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("save_preview_image", { dest: "/tmp/picture.png", base64: btoa("image bytes") }));
});
