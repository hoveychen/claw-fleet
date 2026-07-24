import { describe, expect, it } from "vitest";
import { IMG_ZOOM_INJECT, parseImgZoom } from "./iframeImgZoom";

describe("parseImgZoom", () => {
  it("extracts the src from a well-formed zoom message", () => {
    expect(parseImgZoom({ __fleetImgZoom: "data:image/png;base64,AAAA" })).toBe(
      "data:image/png;base64,AAAA",
    );
    expect(parseImgZoom({ __fleetImgZoom: "https://x/y.png" })).toBe("https://x/y.png");
  });

  it("rejects payloads that are not zoom messages (trust boundary)", () => {
    expect(parseImgZoom(null)).toBeNull();
    expect(parseImgZoom(undefined)).toBeNull();
    expect(parseImgZoom("data:image/png;base64,AAAA")).toBeNull();
    expect(parseImgZoom(42)).toBeNull();
    expect(parseImgZoom({ __fleetAskHeight: 300 })).toBeNull(); // the height message
    expect(parseImgZoom({ __fleetImgZoom: "" })).toBeNull(); // empty src
    expect(parseImgZoom({ __fleetImgZoom: 123 })).toBeNull(); // non-string src
    expect(parseImgZoom({ __fleetImgZoom: null })).toBeNull();
  });

  it("ships an injectable bridge that posts the zoom key", () => {
    // The injected script and the parser must agree on the key, else clicks in
    // the sandbox would post a message the parent silently drops.
    expect(IMG_ZOOM_INJECT).toContain("__fleetImgZoom");
    expect(IMG_ZOOM_INJECT).toContain("parent.postMessage");
    expect(IMG_ZOOM_INJECT).toContain("<script>");
  });
});
