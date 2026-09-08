import { describe, expect, it } from "vitest";

import {
  MAX_RELAY_BYTES,
  formatBytes,
  isFetchable,
  previewKind,
  previewKindFor,
} from "./artifacts";
import type { Artifact } from "./types";

function make(over: Partial<Artifact>): Artifact {
  return {
    id: "20260827-120000",
    name: "a.pdf",
    title: "a.pdf",
    note: "",
    mime: "application/pdf",
    kind: "pdf",
    sizeBytes: 1000,
    createdMs: 1_756_000_000_000,
    workspacePath: "/w",
    workspaceName: "w",
    path: "",
    currentVersion: "v1",
    versions: [
      { id: "v1", addedMs: 1_756_000_000_000, sizeBytes: 1000, sourcePath: "/src/a.pdf", hardlinked: true },
    ],
    sessionId: null,
    sourcePath: "/src/a.pdf",
    starred: false,
    hardlinked: true,
    drifted: false,
    ...over,
  };
}

describe("isFetchable", () => {
  /**
   * The whole phone-side design rests on this line: the relay moves bytes only
   * as one base64 frame, so anything past the cap has to be shown as a card
   * pointing at the desktop rather than attempted and failed.
   */
  it("draws the line exactly at the relay's frame cap", () => {
    expect(isFetchable(make({ sizeBytes: MAX_RELAY_BYTES - 1 }))).toBe(true);
    expect(isFetchable(make({ sizeBytes: MAX_RELAY_BYTES }))).toBe(true);
    expect(isFetchable(make({ sizeBytes: MAX_RELAY_BYTES + 1 }))).toBe(false);
  });

  it("treats a rendered video as out of reach", () => {
    expect(isFetchable(make({ kind: "video", sizeBytes: 412_663_296 }))).toBe(false);
  });
});

describe("previewKind", () => {
  it("previews only what the phone can hold in memory and render", () => {
    expect(previewKind(make({ kind: "image", sizeBytes: 5000 }))).toBe("image");
    expect(previewKind(make({ kind: "pdf", sizeBytes: 5000 }))).toBe("pdf");
    expect(previewKind(make({ kind: "text", sizeBytes: 5000 }))).toBe("text");
    // Archives never get a viewer.
    expect(previewKind(make({ kind: "archive", sizeBytes: 5000 }))).toBe("none");
  });

  /**
   * The three OOXML formats render through the same JS libraries the desktop
   * uses. The match is on mime, not the store's coarse `kind`, because that
   * bucket is wider than what the renderers read: a legacy .doc is a binary
   * CFB container and an .odt is a differently-shaped zip, and handing either
   * to docx-preview throws rather than degrading.
   */
  it("previews OOXML but not the legacy or ODF formats sharing its kind", () => {
    const office = (kind: string, mime: string) =>
      previewKind(make({ kind, mime, sizeBytes: 5000 }));
    expect(
      office("doc", "application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
    ).toBe("docx");
    expect(
      office("sheet", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
    ).toBe("xlsx");
    expect(
      office(
        "slides",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
      ),
    ).toBe("pptx");

    expect(office("doc", "application/msword")).toBe("none");
    expect(office("sheet", "application/vnd.ms-excel")).toBe("none");
    expect(office("slides", "application/vnd.ms-powerpoint")).toBe("none");
    expect(office("doc", "application/vnd.oasis.opendocument.text")).toBe("none");
  });

  /** Size still beats format: an OOXML file over the relay ceiling is unreachable. */
  it("refuses an OOXML file too big for one relay frame", () => {
    expect(
      previewKind(
        make({
          kind: "slides",
          mime: "application/vnd.openxmlformats-officedocument.presentationml.presentation",
          sizeBytes: MAX_RELAY_BYTES + 1,
        }),
      ),
    ).toBe("none");
  });

  /**
   * Size beats kind. A 40 MB PNG is still a PNG, but fetching it means pulling
   * 40 MB through one base64 frame — so the size gate has to run first or the
   * detail view sits on a spinner until the request errors out.
   */
  it("refuses a previewable kind that is too big to fetch", () => {
    expect(previewKind(make({ kind: "image", sizeBytes: MAX_RELAY_BYTES + 1 }))).toBe("none");
    expect(previewKind(make({ kind: "pdf", sizeBytes: MAX_RELAY_BYTES + 1 }))).toBe("none");
  });

  /**
   * The store lumps every `text/*` file into one `text` kind, so before this
   * split a markdown spec previewed as `##` and an html report as its own
   * source. Both are ordinary deliverables — the wiki/artifact routing rule is
   * audience, not file format — so the phone has to tell the three apart.
   */
  it("splits the text kind into markdown, html and plain", () => {
    const text = (mime: string) => previewKind(make({ kind: "text", mime, sizeBytes: 5000 }));
    expect(text("text/markdown; charset=utf-8")).toBe("markdown");
    expect(text("text/html; charset=utf-8")).toBe("html");
    expect(text("text/plain; charset=utf-8")).toBe("text");
    expect(text("text/csv; charset=utf-8")).toBe("text");
    // Parameter and case noise off the wire must not push a real markdown doc
    // back onto the raw-source path.
    expect(text("TEXT/MARKDOWN")).toBe("markdown");
    expect(text(" text/html ")).toBe("html");
  });

  it("still applies the size gate to markup", () => {
    expect(
      previewKind(make({ kind: "text", mime: "text/html", sizeBytes: MAX_RELAY_BYTES + 1 })),
    ).toBe("none");
  });

  it("browses a .zip but not the archives with no directory", () => {
    // A zip carries a central directory, so it can be walked as a folder
    // (shared-ts/zipDir.ts). tar/gz/7z share the store's `archive` kind and
    // cannot be, so they stay on the placeholder.
    expect(previewKind(make({ kind: "archive", mime: "application/zip", sizeBytes: 1000 }))).toBe(
      "zip",
    );
    expect(previewKind(make({ kind: "archive", mime: "application/gzip", sizeBytes: 1000 }))).toBe(
      "none",
    );
    expect(previewKind(make({ kind: "archive", mime: "application/x-tar", sizeBytes: 1000 }))).toBe(
      "none",
    );
  });

  it("keeps the relay ceiling ahead of the zip browser", () => {
    // The phone has no ranged read: an archive it cannot fetch whole is one it
    // cannot browse either, and must keep saying so.
    expect(
      previewKind(
        make({ kind: "archive", mime: "application/zip", sizeBytes: MAX_RELAY_BYTES + 1 }),
      ),
    ).toBe("none");
  });

  it("types a zip member the same way it types an artifact", () => {
    // `previewKindFor` is what the zip browser calls for each member; a
    // report.md must land on the same renderer either way.
    expect(previewKindFor("text", "text/markdown; charset=utf-8")).toBe("markdown");
    expect(previewKindFor("image", "image/png")).toBe("image");
    expect(previewKindFor("other", "application/octet-stream")).toBe("none");
  });

  /** Video and audio are listed but never played here — see the module docs. */
  it("does not offer playback for media", () => {
    expect(previewKind(make({ kind: "video", sizeBytes: 1000 }))).toBe("none");
    expect(previewKind(make({ kind: "audio", sizeBytes: 1000 }))).toBe("none");
  });
});

describe("formatBytes", () => {
  it("matches the desktop's rounding so one artifact reads the same on both", () => {
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(1_468_006)).toBe("1.4 MB");
    expect(formatBytes(412_663_296)).toBe("394 MB");
    expect(formatBytes(0)).toBe("0 B");
  });
});
