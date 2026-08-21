import { describe, expect, it } from "vitest";

import { imageDataUrl, isTrimmedImageData } from "./imageData";
import type { ImageBlock } from "./types";

describe("imageDataUrl", () => {
  it("builds a data: URI for an inline base64 block", () => {
    const block: ImageBlock = {
      type: "image",
      source: { type: "base64", media_type: "image/png", data: "AAAA" },
    };
    expect(imageDataUrl(block)).toBe("data:image/png;base64,AAAA");
  });

  // dsh's transcripts carry a store path, not bytes — the renderer resolves it
  // through the same custom protocol the composer's attachments use.
  it("builds an attachment URL for a store-path block", () => {
    const block: ImageBlock = {
      type: "image",
      source: {
        type: "path",
        media_type: "image/png",
        path: "/Users/u/.fleet/user-attachments/dae52f012e32522b/probe.png",
      },
    };
    expect(imageDataUrl(block)).toBe(
      "fleet-attachment://localhost/dae52f012e32522b/probe.png",
    );
  });

  // A path outside the store has no protocol URL: previewing an arbitrary path
  // off the agent host is not something the renderer is licensed to do.
  it("returns null for a path outside the store", () => {
    const block: ImageBlock = {
      type: "image",
      source: { type: "path", media_type: "image/png", path: "/etc/hosts" },
    };
    expect(imageDataUrl(block)).toBeNull();
  });

  it("returns null for a payload-less block", () => {
    expect(
      imageDataUrl({ type: "image", source: { type: "path", media_type: "image/png" } }),
    ).toBeNull();
    expect(
      imageDataUrl({ type: "image", source: { type: "base64", media_type: "image/png" } }),
    ).toBeNull();
  });

  // The trim marker only ever appears inside inline base64; a path-source block
  // must not be mistaken for a truncated payload and hidden behind the
  // "image truncated" placeholder.
  it("never reports a store-path block as trimmed", () => {
    expect(
      isTrimmedImageData({
        type: "image",
        source: {
          type: "path",
          media_type: "image/png",
          path: "/Users/u/.fleet/user-attachments/ab12cd34ef567890/x.png",
        },
      }),
    ).toBe(false);
  });
});
