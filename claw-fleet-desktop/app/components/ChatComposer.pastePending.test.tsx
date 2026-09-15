// @vitest-environment jsdom
//
// The chip a pasted attachment gets *while its bytes are still moving*.
//
// A pasted screenshot takes seconds to reach the agent: the bytes are read,
// serialized into a JSON number array and handed across the IPC boundary. The
// composer used to render nothing at all until that finished, so a ⌘V looked
// like it had been swallowed and the feature looked broken. These tests pin the
// receipt: a chip in the same frame as the paste, gone once the real attachment
// lands.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// A staging call that resolves only when the test says so — that unresolved
// window is what "still in flight" means here.
let releaseStage: ((path: string) => void) | null = null;
const invokeMock = vi.fn(
  () =>
    new Promise<string>((resolve) => {
      releaseStage = resolve;
    }),
);

vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invokeMock(...(a as [])) }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(async () => null) }));

import "../i18n";
import { ChatComposer } from "./ChatComposer";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
(globalThis as unknown as { ResizeObserver: typeof ResizeObserver }).ResizeObserver = class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

URL.createObjectURL = () => "blob:stub";
URL.revokeObjectURL = () => {};
(globalThis as unknown as { Image: unknown }).Image = class {
  onload: (() => void) | null = null;
  onerror: (() => void) | null = null;
  naturalWidth = 16;
  naturalHeight = 9;
  set src(_v: string) {
    queueMicrotask(() => this.onload?.());
  }
};

let container: HTMLDivElement | null = null;
let root: Root | null = null;

beforeEach(() => {
  vi.clearAllMocks();
  releaseStage = null;
});

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
});

function pasteEvent(files: File[]): Event {
  const ev = new Event("paste", { bubbles: true, cancelable: true });
  Object.defineProperty(ev, "clipboardData", {
    value: {
      items: files.map((f) => ({ kind: "file", type: f.type, getAsFile: () => f })),
      types: files.length ? ["Files"] : [],
      files,
      getData: () => "",
    },
  });
  return ev;
}

function pngFile(name = "image.png"): File {
  const f = new File([new Uint8Array([1, 2, 3])], name, { type: "image/png" });
  // jsdom's File has no arrayBuffer in this environment's older shim path.
  if (!f.arrayBuffer) {
    Object.defineProperty(f, "arrayBuffer", { value: async () => new ArrayBuffer(3) });
  }
  return f;
}

async function mount(props: Partial<Parameters<typeof ChatComposer>[0]> = {}) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  const onAddAttachment = vi.fn();
  await act(async () => {
    root!.render(
      <ChatComposer
        value=""
        onChange={() => {}}
        attachments={[]}
        onAddAttachment={onAddAttachment}
        onRemoveAttachment={() => {}}
        {...props}
      />,
    );
  });
  const textarea = container.querySelector("textarea");
  if (!textarea) throw new Error("composer rendered without a textarea");
  return { onAddAttachment, textarea };
}

function pendingChips(): Element[] {
  return [...(container?.querySelectorAll('[aria-busy="true"]') ?? [])];
}

describe("pasted attachment progress", () => {
  it("shows a chip while the bytes are still being staged", async () => {
    const { textarea } = await mount();

    await act(async () => {
      textarea.dispatchEvent(pasteEvent([pngFile()]));
    });

    // Staging has not resolved, so the real attachment cannot exist yet — the
    // only thing standing between the user and an apparently dead ⌘V.
    expect(invokeMock).toHaveBeenCalledWith("stage_pasted_attachment", expect.anything());
    const chips = pendingChips();
    expect(chips).toHaveLength(1);
    // The object URL is free, so the thumbnail is up before the bytes move.
    expect(chips[0].querySelector("img")?.getAttribute("src")).toBe("blob:stub");
  });

  it("drops the chip once the attachment has been handed to the host", async () => {
    const { onAddAttachment } = await mount();
    const { textarea } = { textarea: container!.querySelector("textarea")! };

    await act(async () => {
      textarea.dispatchEvent(pasteEvent([pngFile()]));
    });
    expect(pendingChips()).toHaveLength(1);

    await act(async () => {
      releaseStage?.("/tmp/staged.png");
      await Promise.resolve();
    });

    expect(onAddAttachment).toHaveBeenCalledTimes(1);
    expect(pendingChips()).toHaveLength(0);
  });

  it("raises one chip per pasted file up front, not one at a time", async () => {
    const { textarea } = await mount();

    await act(async () => {
      textarea.dispatchEvent(pasteEvent([pngFile("a.png"), pngFile("b.png")]));
    });

    // The staging loop is sequential, so only the first file has called invoke —
    // the second file's chip must not wait on it.
    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(pendingChips()).toHaveLength(2);
  });
});
