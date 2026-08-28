import { describe, expect, it } from "vitest";

import { MAX_THUMB_BYTES, officeMode, thumbMode } from "./officePreview";

const DOCX = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

describe("officeMode", () => {
  it("routes the three OOXML formats to their renderers", () => {
    expect(
      officeMode("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
    ).toBe("docx");
    expect(
      officeMode("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
    ).toBe("xlsx");
    expect(
      officeMode("application/vnd.openxmlformats-officedocument.presentationml.presentation"),
    ).toBe("pptx");
  });

  it("refuses everything the three viewers cannot actually read", () => {
    // The store's `doc` / `sheet` / `slides` buckets are wider than OOXML. Each
    // of these would reach a viewer as the same coarse kind and throw there, so
    // the mime split has to keep them on the placeholder.
    for (const mime of [
      "application/msword", // .doc — binary CFB, not a zip of XML
      "application/vnd.ms-excel", // .xls
      "application/vnd.ms-powerpoint", // .ppt
      "application/vnd.oasis.opendocument.text", // .odt — a differently-shaped zip
      "application/vnd.oasis.opendocument.spreadsheet",
      "application/vnd.oasis.opendocument.presentation",
      "application/rtf",
      "application/epub+zip",
      "application/pdf",
      "application/zip",
      "application/octet-stream",
      "text/markdown; charset=utf-8",
      "",
    ]) {
      expect(officeMode(mime), mime).toBeNull();
    }
  });

  it("tolerates the parameter and casing forms a mime can arrive in", () => {
    expect(
      officeMode(
        " APPLICATION/VND.OPENXMLFORMATS-OFFICEDOCUMENT.WORDPROCESSINGML.DOCUMENT; charset=binary ",
      ),
    ).toBe("docx");
  });
});

describe("thumbMode", () => {
  it("stops at the size ceiling so a card never pulls a huge file", () => {
    // A grid card is 190px wide; an 11 MB deck downloaded to fill it is the
    // whole reason this ceiling exists. The detail view has no such cap — it
    // is what the user explicitly asked for.
    expect(thumbMode(DOCX, MAX_THUMB_BYTES)).toBe("docx");
    expect(thumbMode(DOCX, MAX_THUMB_BYTES + 1)).toBeNull();
  });

  it("never offers a thumbnail for a format that has no renderer", () => {
    expect(thumbMode("application/msword", 1000)).toBeNull();
    expect(thumbMode("application/zip", 1000)).toBeNull();
  });
});
