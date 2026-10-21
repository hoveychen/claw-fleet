import { describe, expect, it } from "vitest";
import {
  LIBRARY_CHUNK_BYTES,
  MAX_LIBRARY_UPLOAD_BYTES,
  UploadSkippedAll,
  chunkRanges,
  folderChoices,
  uploadProjects,
  uniqueUploadNames,
  uploadToLibrary,
} from "./libraryApi";
import type { FleetTransport } from "./transport";

type Call = { method: string; params: Record<string, unknown> };

/** A transport that answers the upload methods like the host would and
 *  records every call; `failOnce` makes the first try of matching calls throw. */
function fakeClient(failOnce?: (c: Call) => boolean) {
  const calls: Call[] = [];
  const failed = new Set<string>();
  const client = {
    async request(method: string, params: Record<string, unknown> = {}) {
      const call = { method, params };
      calls.push(call);
      const key = `${method}:${String(params.uploadId)}:${String(params.offset)}`;
      if (failOnce?.(call) && !failed.has(key)) {
        failed.add(key);
        throw new Error("timeout");
      }
      switch (method) {
        case "library_ingest_create":
          return { id: "b1", status: "receiving", items: [] };
        case "library_ingest_chunk":
          return { received: 0 };
        case "library_ingest_start":
          return { id: "b1", status: "queued", items: [] };
        default:
          return {};
      }
    },
  } as unknown as FleetTransport;
  return { client, calls };
}

function file(name: string, size: number): File {
  return new File([new Uint8Array(size).fill(65)], name, { type: "image/jpeg" });
}

describe("chunkRanges", () => {
  it("covers the file in order with no gap or overlap", () => {
    expect(chunkRanges(10, 4)).toEqual([
      [0, 4],
      [4, 8],
      [8, 10],
    ]);
    expect(chunkRanges(8, 4)).toEqual([
      [0, 4],
      [4, 8],
    ]);
  });
  it("sends an empty file as one empty piece", () => {
    expect(chunkRanges(0)).toEqual([[0, 0]]);
  });
});

describe("uniqueUploadNames", () => {
  it("numbers camera shots that all share one name", () => {
    expect(uniqueUploadNames(["image.jpg", "image.jpg", "IMAGE.jpg", "a.pdf"])).toEqual([
      "image.jpg",
      "image (2).jpg",
      "IMAGE (3).jpg",
      "a.pdf",
    ]);
  });
});

describe("uploadToLibrary", () => {
  it("sends each file in offset-tagged pieces, the last one flagged, then starts", async () => {
    const { client, calls } = fakeClient();
    const big = file("scan.pdf", LIBRARY_CHUNK_BYTES + 10);
    const out = await uploadToLibrary(client, "/w/p", [file("image.jpg", 5), big]);
    expect(out.skipped).toEqual([]);
    const chunks = calls.filter((c) => c.method === "library_ingest_chunk").map((c) => c.params);
    expect(chunks.map((c) => [c.path, c.offset, c.last])).toEqual([
      ["image.jpg", 0, true],
      ["scan.pdf", 0, false],
      ["scan.pdf", LIBRARY_CHUNK_BYTES, true],
    ]);
    expect(atob(String(chunks[0].base64))).toBe("AAAAA");
    expect(calls.at(-1)?.method).toBe("library_ingest_start");
  });

  it("resends a piece whose reply was lost, with the same idempotency key", async () => {
    const { client, calls } = fakeClient((c) => c.method === "library_ingest_chunk");
    await uploadToLibrary(client, "/w/p", [file("a.jpg", 3)]);
    const tries = calls.filter((c) => c.method === "library_ingest_chunk");
    expect(tries).toHaveLength(2);
    expect(tries[0].params.idempotencyKey).toBe(tries[1].params.idempotencyKey);
    expect(tries[0].params.offset).toBe(tries[1].params.offset);
  });

  it("skips an oversized file up front and still uploads the rest", async () => {
    const { client } = fakeClient();
    const huge = { name: "movie.mov", size: MAX_LIBRARY_UPLOAD_BYTES + 1 } as File;
    const out = await uploadToLibrary(client, "/w/p", [huge, file("r.jpg", 2)]);
    expect(out.skipped).toEqual([{ name: "movie.mov", reason: "too-large" }]);
  });

  it("throws, without opening a batch, when every file is too large", async () => {
    const { client, calls } = fakeClient();
    const huge = { name: "movie.mov", size: MAX_LIBRARY_UPLOAD_BYTES + 1 } as File;
    await expect(uploadToLibrary(client, "/w/p", [huge])).rejects.toBeInstanceOf(UploadSkippedAll);
    expect(calls).toEqual([]);
  });
});

describe("folderChoices", () => {
  it("lists folders in tree order, indented by depth", () => {
    expect(folderChoices([{ path: "发票/2026" }, { path: "合同" }, { path: "发票" }])).toEqual([
      { path: "发票", label: "发票" },
      { path: "发票/2026", label: "　2026" },
      { path: "合同", label: "合同" },
    ]);
  });
});

describe("uploadProjects", () => {
  it("merges library projects with recent workspaces, library names first", () => {
    expect(
      uploadProjects(
        [{ workspacePath: "/w/house", workspaceName: "house" }],
        [
          { path: "/w/house", name: "house-checkout" },
          { path: "/w/acct", name: "acct" },
        ],
      ),
    ).toEqual([
      { path: "/w/acct", name: "acct" },
      { path: "/w/house", name: "house" },
    ]);
  });
});
