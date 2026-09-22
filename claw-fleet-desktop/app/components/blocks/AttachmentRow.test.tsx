// @vitest-environment jsdom
//
// A replayed user turn's attachments. A file the user picked keeps its own path
// (it is never copied into the store), and the chat bubble used to render it as
// a bare filename chip even though the composer had shown it as a picture a
// moment before send.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.hoisted(() => vi.fn(async () => null as unknown));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import "../../i18n";
import { AttachmentRow } from "./AttachmentRow";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement | null = null;
let root: Root | null = null;

async function mount(paths: string[]) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  await act(async () => {
    root!.render(<AttachmentRow paths={paths} />);
  });
}

const srcs = () => [...container!.querySelectorAll("img")].map((i) => i.getAttribute("src"));
const chips = () => [...container!.querySelectorAll("span[title]")].map((s) => s.getAttribute("title"));

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

describe("AttachmentRow", () => {
  it("serves a store image without reading it back", async () => {
    await mount(["/Users/u/.fleet/user-attachments/ab12/shot.png"]);
    expect(srcs()).toEqual(["fleet-attachment://localhost/ab12/shot.png"]);
    expect(invoke).not.toHaveBeenCalled();
  });

  it("reads a picked host image through read_external_file", async () => {
    invoke.mockResolvedValue({ kind: "image", mime: "image/jpeg", base64: "AAAA" });
    await mount(["/Users/u/Pictures/wife_1.jpg"]);
    expect(invoke).toHaveBeenCalledWith("read_external_file", { path: "/Users/u/Pictures/wife_1.jpg" });
    expect(srcs()).toEqual(["data:image/jpeg;base64,AAAA"]);
    expect(chips()).toEqual([]);
  });

  it("falls back to a chip when the picked file cannot be read", async () => {
    invoke.mockRejectedValue(new Error("not found"));
    await mount(["/Users/u/Pictures/gone.jpg"]);
    expect(srcs()).toEqual([]);
    expect(chips()).toEqual(["/Users/u/Pictures/gone.jpg"]);
  });

  it("never reads a non-image", async () => {
    await mount(["/Users/u/docs/spec.pdf"]);
    expect(invoke).not.toHaveBeenCalled();
    expect(chips()).toEqual(["/Users/u/docs/spec.pdf"]);
  });
});
