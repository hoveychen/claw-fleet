import { describe, expect, it } from "vitest";

import {
  groupIngestItems,
  ingestBatchIdOf,
  ingestBatchKey,
  ingestProgress,
  isIngestRunning,
  uploadRelPath,
  type IngestBatchSummary,
  type IngestItem,
} from "../../shared-ts/libraryIngest";
import { rejectedReasonKey } from "./libraryUpload";

function summary(patch: Partial<IngestBatchSummary>): IngestBatchSummary {
  return {
    id: "b",
    workspacePath: "/p",
    workspaceName: "p",
    status: "done",
    createdMs: 0,
    updatedMs: 0,
    finishedMs: null,
    total: 10,
    extracted: 0,
    classified: 0,
    filed: 0,
    unfiled: 0,
    duplicates: 0,
    failed: 0,
    undone: 0,
    rejected: 0,
    schemaDrafted: false,
    createdFolders: [],
    llmCalls: 0,
    costUsd: 0,
    error: null,
    ...patch,
  };
}

function item(id: string, patch: Partial<IngestItem>): IngestItem {
  return {
    id,
    relPath: `${id}.pdf`,
    name: `${id}.pdf`,
    size: 1,
    sha256: id,
    status: "filed",
    textMethod: "pdf",
    textChars: 0,
    needsVision: false,
    folder: null,
    title: id,
    summary: "",
    tags: [],
    artifactId: `a-${id}`,
    duplicateOf: null,
    error: null,
    ...patch,
  };
}

describe("library upload batches", () => {
  it("measures progress by the counter of the stage in flight", () => {
    expect(ingestProgress(summary({ status: "extracting", extracted: 4 }))).toEqual({ done: 4, total: 10 });
    expect(ingestProgress(summary({ status: "classifying", extracted: 10, classified: 7 }))).toEqual({
      done: 7,
      total: 10,
    });
    expect(ingestProgress(summary({ status: "filing", filed: 5, duplicates: 2 }))).toEqual({ done: 7, total: 10 });
    expect(ingestProgress(summary({ status: "staging", total: 0 }))).toEqual({ done: 0, total: 0 });
  });

  it("knows which statuses are still being worked on", () => {
    expect(["queued", "staging", "extracting", "drafting", "classifying", "filing"].every((s) =>
      isIngestRunning(s as IngestBatchSummary["status"]),
    )).toBe(true);
    // A receiving batch waits on the uploader, not the worker.
    expect(["receiving", "done", "failed", "undone"].some((s) => isIngestRunning(s as IngestBatchSummary["status"]))).toBe(
      false,
    );
  });

  it("groups what went where, unfiled last, duplicates and failures apart", () => {
    const g = groupIngestItems({
      items: [
        item("i1", { folder: "票据" }),
        item("i2", { folder: null }),
        item("i3", { folder: "合同" }),
        item("i4", { folder: "票据", status: "undone" }),
        item("i5", { status: "duplicate", artifactId: null }),
        item("i6", { status: "failed", error: "boom" }),
        item("i7", { status: "extracted" }),
      ],
    });
    expect(g.groups.map((x) => [x.folder, x.items.map((i) => i.id)])).toEqual([
      ["合同", ["i3"]],
      ["票据", ["i1", "i4"]],
      ["", ["i2"]],
    ]);
    expect(g.duplicates.map((i) => i.id)).toEqual(["i5"]);
    expect(g.failed.map((i) => i.id)).toEqual(["i6"]);
    expect(g.pending.map((i) => i.id)).toEqual(["i7"]);
  });

  it("keys a batch apart from library entries and keeps folder-pick paths", () => {
    expect(ingestBatchIdOf(ingestBatchKey("20261021-x"))).toBe("20261021-x");
    expect(ingestBatchIdOf("artifact:20261021-x")).toBeNull();
    expect(ingestBatchIdOf(null)).toBeNull();
    expect(uploadRelPath({ name: "a.pdf", webkitRelativePath: "装修/合同/a.pdf" })).toBe("装修/合同/a.pdf");
    expect(uploadRelPath({ name: "a.pdf", webkitRelativePath: "" })).toBe("a.pdf");
  });

  it("maps core's staging reasons onto locale keys, and leaves unknown ones raw", () => {
    expect(rejectedReasonKey("unsupported archive format (rar) — extract it and upload the files")).toEqual({
      key: "library.reject_unsupported",
      params: { format: "rar" },
    });
    expect(rejectedReasonKey("compression ratio too high (possible zip bomb)").key).toBe("library.reject_bomb");
    expect(rejectedReasonKey("unsafe path inside archive").key).toBe("library.reject_unsafe");
    expect(rejectedReasonKey("over the 1000-file limit for one upload").key).toBe("library.reject_count");
    expect(rejectedReasonKey("over the size limit for one upload").key).toBe("library.reject_total");
    expect(rejectedReasonKey("something new").key).toBe("");
  });
});
