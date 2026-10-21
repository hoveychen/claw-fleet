// The phone's 资料 (library) over the relay: the per-project folder tree,
// index search, folder review and moves, and uploads into the same ingest
// pipeline the desktop's drop and the browser build's picker feed (see
// `claw-fleet-core/src/mobile_relay/library_arms.rs`).
//
// Uploads go in pieces. A relay request is one JSON frame, so a file is read
// a `LIBRARY_CHUNK_BYTES` slice at a time and sent as `library_ingest_chunk`
// with its byte offset; the host appends a piece only at the offset it already
// holds and acknowledges a resend of the piece it just stored, so a retry after
// a lost reply cannot duplicate or skip bytes. The per-file ceiling is the
// ingest pipeline's own (`MAX_UPLOAD_FILE_BYTES`), checked here first so the
// user is told before anything is sent.
//
// Photos travel as taken: no client-side downscale. The upload *is* the
// archived receipt — a reimbursement needs the original — and the vision step
// on the host reads it through Claude, which scales images itself.

import { UPLOAD_REQUEST_TIMEOUT_MS, type FleetTransport } from "./transport";
import type {
  IngestBatch,
  IngestBatchSummary,
} from "../../shared-ts/libraryIngest";
import type {
  LibraryEntry,
  LibraryFolderRec,
  LibraryFolderRename,
  LibraryProjectTree,
} from "../../shared-ts/libraryTree";
import type { LibraryHit } from "../../shared-ts/librarySearch";
import type { Artifact, WikiDoc } from "./types";

export * from "../../shared-ts/libraryTree";
export * from "../../shared-ts/libraryIngest";
export * from "../../shared-ts/librarySearch";
export * from "../../shared-ts/libraryBrowse";

/** Mirrors `library_arms::MAX_CHUNK_BYTES` (the host refuses a larger piece). */
export const LIBRARY_CHUNK_BYTES = 4 * 1024 * 1024;
/** Mirrors `library_ingest::MAX_UPLOAD_FILE_BYTES`. */
export const MAX_LIBRARY_UPLOAD_BYTES = 512 * 1024 * 1024;
/** Tries per piece before a file is given up on. */
const CHUNK_TRIES = 3;

function newKey(): string {
  const c = globalThis.crypto as Crypto | undefined;
  if (c?.randomUUID) return c.randomUUID().replace(/-/g, "");
  return `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 12)}`;
}

// ── Reads ────────────────────────────────────────────────────────────────────

export function fetchLibraryTree(
  client: FleetTransport,
  workspace?: string,
): Promise<LibraryProjectTree[]> {
  return client.request<LibraryProjectTree[]>("library_tree", workspace ? { workspace } : {});
}

/**
 * The tree without the bulk: folders, per-folder counts and the newest
 * `recent` items of each folder, no summaries or tags. The full tree is ~1.4 MB
 * on a real library; a folder's items come from `fetchFolderItems` when it is
 * opened.
 */
export function fetchLibraryOutline(
  client: FleetTransport,
  workspace?: string,
  recent?: number,
): Promise<LibraryProjectTree[]> {
  return client.request<LibraryProjectTree[]>("library_tree", {
    outline: true,
    ...(workspace ? { workspace } : {}),
    ...(recent !== undefined ? { recent } : {}),
  });
}

/** One folder's items and everything beneath it, with summaries and tags;
 *  `folder` `""` = the unfiled ones. */
export function fetchFolderItems(
  client: FleetTransport,
  workspacePath: string,
  folder: string,
): Promise<LibraryEntry[]> {
  return client.request<LibraryEntry[]>("library_folder_items", { workspacePath, folder });
}

/** One wiki doc's record, to open it without pulling `wiki_list`. */
export function fetchWikiDoc(client: FleetTransport, slug: string): Promise<WikiDoc> {
  return client.request<WikiDoc>("wiki_doc", { slug });
}

/** One artifact's record, to open it without pulling `artifact_list`. */
export function fetchArtifactRecord(client: FleetTransport, id: string): Promise<Artifact> {
  return client.request<Artifact>("artifact_get", { id });
}

/** A list-sized JPEG of an image artifact as a data URL, re-encoded on the
 *  host; null when it is not an image or will not decode. */
export async function fetchLibraryThumb(client: FleetTransport, id: string): Promise<string | null> {
  const { thumb } = await client.request<{ thumb: string | null }>("library_thumb", { id });
  return thumb ?? null;
}

export function searchLibrary(
  client: FleetTransport,
  query: string,
  workspace?: string,
): Promise<LibraryHit[]> {
  return client.request<LibraryHit[]>("library_search", {
    query,
    ...(workspace ? { workspace } : {}),
    limit: 40,
  });
}

export function listIngestBatches(
  client: FleetTransport,
  workspace?: string,
): Promise<IngestBatchSummary[]> {
  return client.request<IngestBatchSummary[]>(
    "library_ingest_batches",
    workspace ? { workspace } : {},
  );
}

export function fetchIngestBatch(client: FleetTransport, batchId: string): Promise<IngestBatch> {
  return client.request<IngestBatch>("library_ingest_batch", { batchId });
}

// ── Folder review and moves ─────────────────────────────────────────────────

export function acceptFolder(
  client: FleetTransport,
  workspacePath: string,
  path: string,
): Promise<LibraryFolderRec> {
  return client.request<LibraryFolderRec>("library_folder_accept", { workspacePath, path });
}

export function renameFolder(
  client: FleetTransport,
  workspacePath: string,
  from: string,
  to: string,
): Promise<LibraryFolderRename> {
  return client.request<LibraryFolderRename>("library_folder_rename", {
    workspacePath,
    from,
    to,
    idempotencyKey: newKey(),
  });
}

export async function deleteFolder(
  client: FleetTransport,
  workspacePath: string,
  path: string,
): Promise<void> {
  await client.request("library_folder_delete", {
    workspacePath,
    path,
    idempotencyKey: newKey(),
  });
}

/** File a doc / artifact into a folder of its project; `""` unfiles it. */
export function moveLibraryEntry(
  client: FleetTransport,
  entry: Pick<LibraryEntry, "kind" | "id">,
  folder: string,
): Promise<LibraryEntry> {
  return client.request<LibraryEntry>("library_move", {
    kind: entry.kind,
    id: entry.id,
    folder,
  });
}

// ── Upload batches ──────────────────────────────────────────────────────────

export function undoIngest(
  client: FleetTransport,
  batchId: string,
  itemId?: string,
): Promise<IngestBatch> {
  return client.request<IngestBatch>("library_ingest_undo", {
    batchId,
    ...(itemId ? { itemId } : {}),
    idempotencyKey: newKey(),
  });
}

export function moveIngestItem(
  client: FleetTransport,
  batchId: string,
  itemId: string,
  folder: string,
): Promise<IngestBatch> {
  return client.request<IngestBatch>("library_ingest_move", { batchId, itemId, folder });
}

export async function dismissIngest(client: FleetTransport, batchId: string): Promise<void> {
  await client.request("library_ingest_dismiss", { batchId, idempotencyKey: newKey() });
}

/** `[start, end)` byte ranges a file of `size` is sent in. An empty file is
 *  one empty piece, so it still arrives (and is reported) as a file. */
export function chunkRanges(size: number, chunk = LIBRARY_CHUNK_BYTES): Array<[number, number]> {
  if (size <= 0) return [[0, 0]];
  const out: Array<[number, number]> = [];
  for (let start = 0; start < size; start += chunk) out.push([start, Math.min(size, start + chunk)]);
  return out;
}

/**
 * Names a batch's files will carry, made unique: a phone camera names every
 * shot `image.jpg`, and three receipts all called that read as one file in the
 * batch summary. A clash gets ` (2)`, ` (3)` before its extension.
 */
export function uniqueUploadNames(names: string[]): string[] {
  const seen = new Set<string>();
  return names.map((raw) => {
    const name = raw.trim() || "file";
    if (!seen.has(name.toLowerCase())) {
      seen.add(name.toLowerCase());
      return name;
    }
    const dot = name.lastIndexOf(".");
    const stem = dot > 0 ? name.slice(0, dot) : name;
    const ext = dot > 0 ? name.slice(dot) : "";
    for (let n = 2; ; n++) {
      const next = `${stem} (${n})${ext}`;
      if (!seen.has(next.toLowerCase())) {
        seen.add(next.toLowerCase());
        return next;
      }
    }
  });
}

async function blobBase64(blob: Blob): Promise<string> {
  const bytes = new Uint8Array(await blob.arrayBuffer());
  let bin = "";
  for (let i = 0; i < bytes.length; i += 0x8000) {
    bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  }
  return btoa(bin);
}

export interface UploadProgress {
  /** Index of the file being sent. */
  file: number;
  files: number;
  /** Bytes sent across every file so far. */
  sentBytes: number;
  totalBytes: number;
}

export interface UploadOutcome {
  batch: IngestBatch;
  /** Files that were not sent, with why — the rest of the batch still went. */
  skipped: Array<{ name: string; reason: string }>;
}

/**
 * Send `files` into a new batch of `workspacePath` and start filing it.
 *
 * A file the pipeline would refuse for size is skipped before sending; a file
 * whose piece still fails after `CHUNK_TRIES` is skipped too, and the batch
 * goes ahead with what arrived. Only when nothing arrived does it throw (and
 * the empty batch is dismissed).
 */
export async function uploadToLibrary(
  client: FleetTransport,
  workspacePath: string,
  files: File[],
  onProgress?: (p: UploadProgress) => void,
): Promise<UploadOutcome> {
  const names = uniqueUploadNames(files.map((f) => f.name));
  const skipped: UploadOutcome["skipped"] = [];
  const sendable: Array<{ file: File; name: string }> = [];
  files.forEach((file, i) => {
    if (file.size > MAX_LIBRARY_UPLOAD_BYTES) {
      skipped.push({ name: names[i], reason: "too-large" });
    } else {
      sendable.push({ file, name: names[i] });
    }
  });
  if (sendable.length === 0) throw new UploadSkippedAll(skipped);

  const created = await client.request<IngestBatch>("library_ingest_create", {
    workspacePath,
    idempotencyKey: newKey(),
  });
  const totalBytes = sendable.reduce((s, f) => s + f.file.size, 0);
  let sentBytes = 0;
  let arrived = 0;
  for (let i = 0; i < sendable.length; i++) {
    const { file, name } = sendable[i];
    const uploadId = newKey();
    const ranges = chunkRanges(file.size);
    let failed: string | null = null;
    let fileSent = 0;
    for (let r = 0; r < ranges.length && !failed; r++) {
      const [start, end] = ranges[r];
      const base64 = await blobBase64(file.slice(start, end));
      const last = r === ranges.length - 1;
      for (let attempt = 1; ; attempt++) {
        try {
          await client.request(
            "library_ingest_chunk",
            {
              batchId: created.id,
              uploadId,
              path: name,
              offset: start,
              base64,
              last,
              idempotencyKey: `${uploadId}-${start}`,
            },
            UPLOAD_REQUEST_TIMEOUT_MS,
          );
          break;
        } catch (e) {
          if (attempt >= CHUNK_TRIES) {
            failed = e instanceof Error ? e.message : String(e);
            break;
          }
        }
      }
      if (!failed) {
        fileSent += end - start;
        sentBytes += end - start;
        onProgress?.({ file: i, files: sendable.length, sentBytes, totalBytes });
      }
    }
    if (failed) {
      skipped.push({ name, reason: failed });
      // The bar counts a given-up file as done, so it still reaches the end.
      sentBytes += file.size - fileSent;
    } else {
      arrived += 1;
    }
  }
  if (arrived === 0) {
    await dismissIngest(client, created.id).catch(() => undefined);
    throw new UploadSkippedAll(skipped);
  }
  const batch = await client.request<IngestBatch>("library_ingest_start", {
    batchId: created.id,
    idempotencyKey: newKey(),
  });
  return { batch, skipped };
}

/** Nothing of an upload arrived; `skipped` says why for each file. */
export class UploadSkippedAll extends Error {
  constructor(public skipped: Array<{ name: string; reason: string }>) {
    super(skipped.map((s) => `${s.name}: ${s.reason}`).join("; ") || "nothing to upload");
  }
}

/** A project's folders as picker options: tree order, each indented by depth
 *  so a native `<select>` (the phone's wheel picker) still reads as a tree. */
export function folderChoices(
  folders: ReadonlyArray<Pick<LibraryFolderRec, "path">>,
): Array<{ path: string; label: string }> {
  return [...new Set(folders.map((f) => f.path).filter(Boolean))]
    .sort((a, b) => a.localeCompare(b, undefined, { numeric: true }))
    .map((path) => {
      const depth = path.split("/").length - 1;
      const leaf = path.slice(path.lastIndexOf("/") + 1);
      return { path, label: `${"　".repeat(depth)}${leaf}` };
    });
}

/** Projects to upload into: those the library already knows, then the ones
 *  sessions ran in recently — a project with nothing filed yet is exactly
 *  where a first receipt goes. Library names win; sorted by name. */
export function uploadProjects(
  library: ReadonlyArray<{ workspacePath: string; workspaceName: string }>,
  recent: ReadonlyArray<{ path: string; name: string }>,
): Array<{ path: string; name: string }> {
  const by = new Map<string, string>();
  for (const p of library) if (p.workspacePath) by.set(p.workspacePath, p.workspaceName || p.workspacePath);
  for (const r of recent) if (r.path && !by.has(r.path)) by.set(r.path, r.name || r.path);
  return [...by]
    .map(([path, name]) => ({ path, name }))
    .sort((a, b) => a.name.localeCompare(b.name));
}
