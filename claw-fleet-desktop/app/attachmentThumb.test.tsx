// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import { storeThumbUrl, useAttachmentThumb } from "./attachmentThumb";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const STORE = "/home/u/.fleet/user-attachments/ab12cd34ef567890/lego_1.jpg";

function Probe(props: { path: string; name: string; previewUrl?: string }) {
  const src = useAttachmentThumb(props);
  return <span data-testid="src">{src ?? "none"}</span>;
}

let host: HTMLDivElement;
let root: Root;

async function mount(props: { path: string; name: string; previewUrl?: string }) {
  await act(async () => {
    root.render(<Probe {...props} />);
  });
}

function src(): string {
  return host.querySelector('[data-testid="src"]')?.textContent ?? "";
}

beforeEach(() => {
  invoke.mockReset();
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
});

afterEach(async () => {
  await act(async () => root.unmount());
  host.remove();
});

describe("storeThumbUrl", () => {
  // The browser build uploads a picked file into the store precisely because it
  // has no host path to hand the agent — so its path is always resolvable and
  // its thumbnail needs no bytes over the transport.
  it("resolves a store path for an image", () => {
    expect(storeThumbUrl({ path: STORE, name: "lego_1.jpg" })).toBe(
      "fleet-attachment://localhost/ab12cd34ef567890/lego_1.jpg",
    );
  });

  it("refuses a non-image name and a path outside the store", () => {
    expect(storeThumbUrl({ path: STORE, name: "notes.pdf" })).toBeNull();
    expect(storeThumbUrl({ path: "/Users/me/pics/lego_1.jpg", name: "lego_1.jpg" })).toBeNull();
  });
});

describe("useAttachmentThumb", () => {
  it("prefers a paste's own object URL and reads nothing", async () => {
    await mount({ path: "/tmp/T/fleet-pasted/p.png", name: "p.png", previewUrl: "blob:xyz" });
    expect(src()).toBe("blob:xyz");
    expect(invoke).not.toHaveBeenCalled();
  });

  it("uses the store URL without touching the transport", async () => {
    await mount({ path: STORE, name: "lego_1.jpg" });
    expect(src()).toBe("fleet-attachment://localhost/ab12cd34ef567890/lego_1.jpg");
    expect(invoke).not.toHaveBeenCalled();
  });

  // The desktop picker hands back the file's own path, which is in no store —
  // this is the case that made "picked on the desktop" look identical to the
  // reported cloud bug.
  it("reads an arbitrary host path into a data URL", async () => {
    invoke.mockResolvedValue({ kind: "image", base64: "QUJD", mime: "image/jpeg", sizeBytes: 3 });
    await mount({ path: "/Users/me/pics/lego_1.jpg", name: "lego_1.jpg" });
    expect(invoke).toHaveBeenCalledWith("read_external_file", {
      path: "/Users/me/pics/lego_1.jpg",
    });
    expect(src()).toBe("data:image/jpeg;base64,QUJD");
  });

  // A 40 MiB zip must not be hauled across the transport just to be told it is
  // not a picture.
  it("never reads a file whose name is not an image", async () => {
    await mount({ path: "/Users/me/docs/spec.pdf", name: "spec.pdf" });
    expect(src()).toBe("none");
    expect(invoke).not.toHaveBeenCalled();
  });

  it("stays silent when the read fails", async () => {
    invoke.mockRejectedValue(new Error("gone"));
    await mount({ path: "/Users/me/pics/gone.png", name: "gone.png" });
    expect(invoke).toHaveBeenCalled();
    expect(src()).toBe("none");
  });
});
