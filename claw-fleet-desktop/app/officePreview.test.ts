import { describe, expect, it } from "vitest";

import { officeMode } from "./officePreview";

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
