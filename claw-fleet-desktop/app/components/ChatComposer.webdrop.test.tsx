// @vitest-environment jsdom
//
// Dragging a file onto the composer in the browser build.
//
// On the desktop this never goes through the DOM at all: Tauri intercepts the
// webview's native drop (`dragDropEnabled` defaults true) and hands the *paths*
// over an app event, so ChatComposer listens on `onDragDropEvent` and has no
// React `onDrop` handler. In a tab nothing intercepts anything, that listener's
// `invoke` has no transport, and the absent `onDrop` means a dropped file is
// simply swallowed by the page — no chip, no error, nothing.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => null) }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(async () => null) }));
// The upload itself is `webAttachments`' job and is covered by its own tests;
// what this file pins is that a drop *reaches* it and that its result reaches
// the host.
vi.mock("../webAttachments", () => ({
  uploadPickedFiles: vi.fn(async (files: File[]) =>
    files.map((f) => `/home/u/.fleet/user-attachments/k/${f.name}`),
  ),
  pickAndUploadFiles: vi.fn(async () => null),
}));

import "../i18n";
import { markWebBuild } from "../hostEnv";
import { ChatComposer } from "./ChatComposer";
import { uploadPickedFiles } from "../webAttachments";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
(globalThis as unknown as { ResizeObserver: typeof ResizeObserver }).ResizeObserver = class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

let container: HTMLDivElement | null = null;
let root: Root | null = null;

beforeEach(() => {
  markWebBuild();
  vi.clearAllMocks();
});

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
});

/** jsdom has no `DragEvent`, and React reads `dataTransfer` off the native
 *  event — so a plain Event carrying that one property is enough. */
function dropEvent(type: string, files: File[]): Event {
  const ev = new Event(type, { bubbles: true, cancelable: true });
  Object.defineProperty(ev, "dataTransfer", {
    value: { files, items: files.map((f) => ({ kind: "file", getAsFile: () => f })), types: ["Files"] },
  });
  return ev;
}

async function mount(props: Partial<Parameters<typeof ChatComposer>[0]> = {}) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  const onDropFiles = vi.fn();
  const onAttachmentError = vi.fn();
  await act(async () => {
    root!.render(
      <ChatComposer
        value=""
        onChange={() => {}}
        attachments={[]}
        onAddAttachment={() => {}}
        onRemoveAttachment={() => {}}
        onDropFiles={onDropFiles}
        onAttachmentError={onAttachmentError}
        {...props}
      />,
    );
  });
  return { onDropFiles, onAttachmentError };
}

describe("ChatComposer drop in the browser build", () => {
  it("uploads the dropped files and hands their store paths to the host", async () => {
    const { onDropFiles, onAttachmentError } = await mount();
    const file = new File([new Uint8Array([1, 2])], "shot.png", { type: "image/png" });

    await act(async () => {
      container!.firstElementChild!.dispatchEvent(dropEvent("drop", [file]));
    });

    expect(uploadPickedFiles).toHaveBeenCalledTimes(1);
    expect(onDropFiles).toHaveBeenCalledWith(["/home/u/.fleet/user-attachments/k/shot.png"]);
    expect(onAttachmentError).not.toHaveBeenCalled();
  });

  /**
   * Without `preventDefault` on dragover the browser never fires `drop` at all —
   * it navigates to the file instead, blowing the whole app away. That is the
   * one part of HTML5 drag-and-drop that fails silently in the *other*
   * direction, so it is pinned rather than assumed.
   */
  it("claims the dragover so the browser does not navigate to the file", async () => {
    await mount();
    const ev = dropEvent("dragover", []);
    await act(async () => {
      container!.firstElementChild!.dispatchEvent(ev);
    });
    expect(ev.defaultPrevented).toBe(true);
  });

  it("reports a failed upload instead of dropping the file silently", async () => {
    vi.mocked(uploadPickedFiles).mockRejectedValueOnce(new Error("HTTP 413: too large"));
    const { onDropFiles, onAttachmentError } = await mount();

    await act(async () => {
      container!.firstElementChild!.dispatchEvent(
        dropEvent("drop", [new File([new Uint8Array([1])], "big.bin")]),
      );
    });

    expect(onDropFiles).not.toHaveBeenCalled();
    expect(onAttachmentError).toHaveBeenCalledWith(expect.stringContaining("413"));
  });

  // Mirrors the Tauri path's `if (!onDropFilesRef.current) return` — a composer
  // that does not accept drops must not start uploading bytes anyway.
  it("ignores a drop when the host accepts none", async () => {
    await mount({ onDropFiles: undefined });
    await act(async () => {
      container!.firstElementChild!.dispatchEvent(
        dropEvent("drop", [new File([new Uint8Array([1])], "x.txt")]),
      );
    });
    expect(uploadPickedFiles).not.toHaveBeenCalled();
  });
});
