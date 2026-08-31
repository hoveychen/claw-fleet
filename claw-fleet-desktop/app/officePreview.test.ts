import { describe, expect, it } from "vitest";

import {
  MAX_PDF_THUMB_BYTES,
  MAX_THUMB_BYTES,
  MAX_VIDEO_DOWNLOAD_BYTES,
  MAX_VIDEO_THUMB_BYTES,
  officeMode,
  textPreviewMode,
  thumbMode,
} from "./officePreview";

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

describe("textPreviewMode", () => {
  /**
   * The store lumps every `text/*` file into one `text` kind, so before this
   * split a markdown spec rendered as `##` and an html report rendered as its
   * own source. Both are ordinary deliverables — the wiki/artifact routing rule
   * is audience, not file format — so the stage has to tell the three apart.
   */
  it("separates markdown and html from plain text", () => {
    expect(textPreviewMode("text/markdown; charset=utf-8")).toBe("markdown");
    expect(textPreviewMode("text/html; charset=utf-8")).toBe("html");
    expect(textPreviewMode("text/plain; charset=utf-8")).toBe("plain");
    expect(textPreviewMode("text/csv; charset=utf-8")).toBe("plain");
    // application/json is bucketed `text` by the store but is not markup.
    expect(textPreviewMode("application/json")).toBe("plain");
  });

  it("ignores parameter and case noise in the mime", () => {
    // The value comes off the wire; a stricter equality check would silently
    // send a real markdown doc back to the <pre> path.
    expect(textPreviewMode("TEXT/MARKDOWN")).toBe("markdown");
    expect(textPreviewMode(" text/html ")).toBe("html");
    expect(textPreviewMode("")).toBe("plain");
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
    expect(thumbMode("audio/mpeg", 1000)).toBeNull();
  });

  /**
   * PDF and video are *card* modes with no `officeMode` behind them — the stage
   * hands both to the webview's own viewer, so a shared "is there a renderer"
   * question would answer no for them. The size ceilings are per-format because
   * the three cost models differ (unzip / parse / ranged read), and a single
   * 4 MB cap would leave the exact icon wall this exists to remove: a 5.7 MB
   * render and a 12 MB scanned report are both ordinary here.
   */
  it("draws PDFs and videos too, each against its own ceiling", () => {
    expect(thumbMode("application/pdf", 1000)).toBe("pdf");
    expect(thumbMode("application/pdf", MAX_PDF_THUMB_BYTES)).toBe("pdf");
    expect(thumbMode("application/pdf", MAX_PDF_THUMB_BYTES + 1)).toBeNull();
    // Over the OOXML ceiling but well under the PDF one — the case a single
    // shared cap would get wrong.
    expect(thumbMode("application/pdf", MAX_THUMB_BYTES + 1)).toBe("pdf");

    // A frame grab is a ranged read, so the video ceiling only guards the
    // non-faststart mp4 whose moov atom sits at the tail.
    expect(thumbMode("video/mp4", 5_700_000)).toBe("video");
    expect(thumbMode("video/quicktime", MAX_VIDEO_THUMB_BYTES)).toBe("video");
    expect(thumbMode("video/mp4", MAX_VIDEO_THUMB_BYTES + 1)).toBeNull();
  });

  /**
   * Regression: the fallback ceiling was first wired to `MAX_THUMB_BYTES`, and
   * the 5.7 MB render that motivated this feature is over that — so on any host
   * where the ranged grab taints the canvas, the one clip we were trying to fix
   * would have kept its icon. The fallback costs a whole download, which is the
   * PDF trade, so it takes the PDF ceiling.
   */
  it("lets the video fallback download the clips this feature exists for", () => {
    expect(MAX_VIDEO_DOWNLOAD_BYTES).toBeGreaterThan(MAX_THUMB_BYTES);
    expect(MAX_VIDEO_DOWNLOAD_BYTES).toBeGreaterThanOrEqual(6_016_675);
    // Still bounded — it is a download, not a ranged read.
    expect(MAX_VIDEO_DOWNLOAD_BYTES).toBeLessThan(MAX_VIDEO_THUMB_BYTES);
  });

  /**
   * The grid's remaining icon wall is markdown and html — a `fleet artifact add
   * report.md` lands in the same `text` bucket as a log dump and got the same
   * gray placeholder, even though the stage has rendered both properly since
   * v2.4.0. Both are cheap: no zip to unpack, no page to rasterise.
   */
  it("draws markdown and html cards rather than the generic text icon", () => {
    expect(thumbMode("text/markdown; charset=utf-8", 57_000)).toBe("markdown");
    expect(thumbMode("text/html; charset=utf-8", 12_000)).toBe("html");
    expect(thumbMode(" TEXT/MARKDOWN ", 1000)).toBe("markdown");
  });

  it("leaves the rest of text/* on its icon", () => {
    // A log or a csv has no shape worth shrinking into 190px — rendering one
    // would be a wall of gray lines, which is what the icon already says.
    expect(thumbMode("text/plain; charset=utf-8", 1000)).toBeNull();
    expect(thumbMode("text/csv; charset=utf-8", 1000)).toBeNull();
    expect(thumbMode("application/json", 1000)).toBeNull();
  });

  it("holds text to the same whole-download ceiling as the OOXML three", () => {
    expect(thumbMode("text/markdown", MAX_THUMB_BYTES)).toBe("markdown");
    expect(thumbMode("text/markdown", MAX_THUMB_BYTES + 1)).toBeNull();
    expect(thumbMode("text/html", MAX_THUMB_BYTES + 1)).toBeNull();
  });

  it("tolerates the codec parameters a media mime arrives with", () => {
    // The store writes the mime it derived; a `video/mp4; codecs="avc1.4d401e"`
    // matched by equality would silently fall through to the office branch.
    expect(thumbMode('VIDEO/MP4; codecs="avc1.4d401e"', 1000)).toBe("video");
    expect(thumbMode(" application/pdf ", 1000)).toBe("pdf");
  });
});
