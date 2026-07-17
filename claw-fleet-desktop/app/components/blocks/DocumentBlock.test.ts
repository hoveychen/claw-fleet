import { describe, expect, it } from "vitest";
import { documentInfo, formatBytes } from "./DocumentBlock";
import type { ContentBlock } from "../../types";

describe("formatBytes", () => {
  it("bytes under 1 KB read raw", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(512)).toBe("512 B");
  });

  it("KB range rounds to whole KB", () => {
    expect(formatBytes(1024)).toBe("1 KB");
    expect(formatBytes(348_160)).toBe("340 KB");
  });

  it("MB range keeps one decimal under 10 MB, none above", () => {
    expect(formatBytes(1_258_291)).toBe("1.2 MB");
    expect(formatBytes(12 * 1024 * 1024)).toBe("12 MB");
  });
});

describe("documentInfo", () => {
  it("derives an uppercase label from the media subtype + decodes base64 size", () => {
    // "SGVsbG8=" is 8 base64 chars with one '=' pad → 5 bytes ("Hello").
    const block = {
      type: "document",
      source: { type: "base64", media_type: "application/pdf", data: "SGVsbG8=" },
    } as unknown as ContentBlock;
    expect(documentInfo(block)).toEqual({ label: "PDF", bytes: 5 });
  });

  it("falls back to 'Document' with null size when media type / data absent", () => {
    const block = { type: "document", source: { type: "url", url: "http://x/y.pdf" } } as unknown as ContentBlock;
    expect(documentInfo(block)).toEqual({ label: "Document", bytes: null });
  });

  it("tolerates a missing source entirely", () => {
    expect(documentInfo({ type: "document" } as unknown as ContentBlock)).toEqual({
      label: "Document",
      bytes: null,
    });
  });
});
