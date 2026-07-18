// @vitest-environment jsdom
// A tool-result image that fails to load must leave a *visible* failure state,
// not vanish. The old behavior (`broken → return null`) rendered every
// upstream transport fault — a trimmed base64, a missing refetch, a dropped
// flag — as the same silent empty box, which is how the "Read 图片不显示" bug
// survived three transport-layer fixes: the frontend erased the evidence.
import { afterEach, describe, expect, it } from "vitest";
import { createElement } from "react";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";

import "../../i18n";
import { ImageThumb } from "./ImageThumb";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement | null = null;
let root: Root | null = null;
afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
});

function mount(node: React.ReactElement) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(node));
}

describe("ImageThumb failure visibility", () => {
  it("shows a visible failure state instead of vanishing when the image errors", async () => {
    mount(
      createElement(ImageThumb, {
        block: {
          type: "image",
          source: { type: "base64", media_type: "image/png", data: "not-base64!!" },
        } as never,
        alt: "tool image",
      }),
    );

    const img = container!.querySelector("img");
    expect(img, "thumb <img> should render initially").toBeTruthy();

    // jsdom never decodes images, so drive the error path explicitly — this is
    // exactly what WebKit does with an invalid data URI.
    await act(async () => {
      img!.dispatchEvent(new Event("error"));
    });

    // The old code rendered nothing here. The card must keep a visible,
    // labeled failure element so the reader knows an image failed to display.
    expect(container!.textContent, "failure state must be visible").not.toBe("");
    expect(
      container!.querySelector("[data-testid='image-thumb-broken']"),
      "broken-image fallback element must exist",
    ).toBeTruthy();
  });

  it("shows a truncation placeholder for transport-trimmed base64 instead of a doomed <img>", () => {
    // The Rust transport trim rewrites long string leaves — including an image
    // source's base64 — to `<1KB prefix>\n\n…[Fleet truncated N bytes — …]`.
    // That string can never decode; rendering it as an <img> guarantees the
    // silent-empty failure. The thumb must recognize the marker up front.
    mount(
      createElement(ImageThumb, {
        block: {
          type: "image",
          source: {
            type: "base64",
            media_type: "image/jpeg",
            data: "/9j/4AAQSkZJRg\n\n…[Fleet truncated 195720 bytes — expand to load full output]",
          },
        } as never,
        alt: "tool image",
      }),
    );

    expect(
      container!.querySelector("[data-testid='image-thumb-truncated']"),
      "trimmed base64 must render as an explicit truncation placeholder",
    ).toBeTruthy();
    expect(container!.querySelector("img"), "no <img> for known-corrupt data").toBeFalsy();
  });
});
