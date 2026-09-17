import { describe, expect, it } from "vitest";

import { classifyScan } from "./scanAvailability";

describe("classifyScan", () => {
  it("secure context + has getUserMedia = can scan", () => {
    expect(classifyScan({ secureContext: true, hasGetUserMedia: true })).toBe("ok");
  });

  it("insecure context is its own bucket, not grouped as 'no camera'", () => {
    // On http pages mediaDevices doesn't exist at all, so this input combo is the real
    // http scenario. Misclassifying forces the user to dig in system settings for a
    // switch that doesn't exist.
    expect(classifyScan({ secureContext: false, hasGetUserMedia: false })).toBe("insecure-origin");
  });

  it("secure context but no getUserMedia = this device has no camera API", () => {
    expect(classifyScan({ secureContext: true, hasGetUserMedia: false })).toBe("no-camera-api");
  });
});
