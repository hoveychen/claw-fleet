import { describe, expect, it } from "vitest";

import { MAX_RELAY_BYTES, formatBytes, isFetchable, previewKind } from "./artifacts";
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
    // Office and archives have no viewer on either host.
    expect(previewKind(make({ kind: "sheet", sizeBytes: 5000 }))).toBe("none");
    expect(previewKind(make({ kind: "archive", sizeBytes: 5000 }))).toBe("none");
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
