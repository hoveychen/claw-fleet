import { describe, expect, it } from "vitest";
import { imageFileName } from "./imageSave";

const NOW = new Date(2026, 8, 28, 7, 5, 9);

describe("imageFileName", () => {
  it("keeps the filename of an http(s) src that has an extension", () => {
    expect(imageFileName("https://x.test/a/b/shot%201.png?v=2", "image/png", NOW)).toBe("shot 1.png");
  });

  it("generates a stamped name from the MIME type for data: and blob: srcs", () => {
    expect(imageFileName("data:image/jpeg;base64,AAAA", "image/jpeg", NOW)).toBe(
      "image-20260928-070509.jpg",
    );
    expect(imageFileName("blob:https://x.test/uuid", "image/webp", NOW)).toBe(
      "image-20260928-070509.webp",
    );
  });

  it("falls back to png for an unknown or empty MIME type", () => {
    expect(imageFileName("data:,", "", NOW)).toBe("image-20260928-070509.png");
    expect(imageFileName("https://x.test/render", "application/octet-stream", NOW)).toBe(
      "image-20260928-070509.png",
    );
  });
});
