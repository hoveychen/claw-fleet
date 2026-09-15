import { describe, expect, it } from "vitest";

import { classifyScan } from "./scanAvailability";

describe("classifyScan", () => {
  it("安全上下文 + 有 getUserMedia = 能扫", () => {
    expect(classifyScan({ secureContext: true, hasGetUserMedia: true })).toBe("ok");
  });

  it("非安全上下文单独成一档，而不是被归成「没有摄像头」", () => {
    // http 页面里 mediaDevices 整个不存在,所以这一组入参就是真实的 http 现场。
    // 归错档的代价是让用户去系统设置里找一个不存在的开关。
    expect(classifyScan({ secureContext: false, hasGetUserMedia: false })).toBe("insecure-origin");
  });

  it("安全上下文但没有 getUserMedia = 这台设备用不了摄像头", () => {
    expect(classifyScan({ secureContext: true, hasGetUserMedia: false })).toBe("no-camera-api");
  });
});
