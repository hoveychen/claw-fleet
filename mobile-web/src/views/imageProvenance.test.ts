import { describe, expect, it } from "vitest";
import { parseImageProvenance } from "./fleetTools";

describe("parseImageProvenance", () => {
  it("reads the line the relay forwards as `_image.prov`", () => {
    expect(
      parseImageProvenance("gpt-image-2.5-flare via chatgpt, quality medium, size 1536x1024, background opaque"),
    ).toEqual({
      model: "gpt-image-2.5-flare",
      route: "chatgpt",
      quality: "medium",
      size: "1536x1024",
      background: "opaque",
      requested: false,
    });
  });

  it("flags requested-only values and ignored controls", () => {
    expect(parseImageProvenance("m1 via api-key, requested size 1024x1024 — quality ignored by this backend")).toEqual({
      model: "m1",
      route: "api-key",
      size: "1024x1024",
      requested: true,
      ignored: "quality",
    });
    expect(parseImageProvenance("garbage")).toBeNull();
  });
});
