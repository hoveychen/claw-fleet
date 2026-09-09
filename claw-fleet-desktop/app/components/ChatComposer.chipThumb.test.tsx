// @vitest-environment jsdom
//
// The attachment chip's thumbnail, for the attachments that arrive as a bare
// `{path, name}`.
//
// Only a paste ever carried a `previewUrl` (an object URL made from the `File`
// it arrives as); the ＋ picker, both drops and a decision card's pick all hand
// the composer a path and nothing else, so a picked image rendered as a
// filename chip while a pasted one rendered as a picture. In the browser build
// picking is the only practical way in, which is why it read as "the cloud
// dropped my image".
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.hoisted(() => vi.fn(async () => null as unknown));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(async () => null) }));

import "../i18n";
import { ChatComposer } from "./ChatComposer";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
(globalThis as unknown as { ResizeObserver: typeof ResizeObserver }).ResizeObserver = class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

let container: HTMLDivElement | null = null;
let root: Root | null = null;

async function mount(attachments: { path: string; name: string; previewUrl?: string }[]) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  await act(async () => {
    root!.render(
      <ChatComposer
        value=""
        onChange={() => {}}
        attachments={attachments}
        onAddAttachment={() => {}}
        onRemoveAttachment={() => {}}
      />,
    );
  });
}

function thumbs(): string[] {
  return [...container!.querySelectorAll("img")].map((i) => i.getAttribute("src") ?? "");
}

beforeEach(() => {
  invoke.mockReset();
  invoke.mockResolvedValue(null);
});

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
});

describe("attachment chip thumbnails", () => {
  // What the browser build's picker produces: the bytes were uploaded into the
  // store, so the path alone is enough to render it.
  it("shows a picked store image without reading it back", async () => {
    await mount([
      { path: "/home/u/.fleet/user-attachments/ab12/lego_1.jpg", name: "lego_1.jpg" },
    ]);
    expect(thumbs()).toEqual(["fleet-attachment://localhost/ab12/lego_1.jpg"]);
    expect(invoke).not.toHaveBeenCalled();
  });

  // What the desktop picker produces: the file keeps its own path, so the bytes
  // come back through the same door agent-written markdown images use.
  it("reads a picked host image through read_external_file", async () => {
    invoke.mockResolvedValue({
      kind: "image",
      base64: "QUJD",
      mime: "image/jpeg",
      sizeBytes: 3,
    });
    await mount([{ path: "/Users/me/pics/lego_1.jpg", name: "lego_1.jpg" }]);
    expect(invoke).toHaveBeenCalledWith("read_external_file", {
      path: "/Users/me/pics/lego_1.jpg",
    });
    expect(thumbs()).toEqual(["data:image/jpeg;base64,QUJD"]);
  });

  // A chip for a non-image keeps its old shape — and costs no transport round
  // trip to find that out.
  it("leaves a non-image chip alone", async () => {
    await mount([{ path: "/Users/me/docs/spec.pdf", name: "spec.pdf" }]);
    expect(thumbs()).toEqual([]);
    expect(container!.textContent).toContain("spec.pdf");
    expect(invoke).not.toHaveBeenCalled();
  });
});
