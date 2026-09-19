// Artifact-store client for the mobile web app. Deliverables are stored on the
// desktop into ~/.fleet/artifacts and read over the relay via `artifact_list` /
// `artifact_blob` (see claw-fleet-core/src/mobile_relay.rs).
//
// Two ways to get bytes, and the difference is the whole of this file's design.
// `fetchArtifact` is one base64 payload in one JSON frame, which is what every
// preview uses and why previews stop at `MAX_RELAY_BYTES` — a preview has to
// fit in memory anyway. `downloadArtifact` walks the same method by byte range,
// so the limit on a download is the device, not the wire; that is what lets a
// rendered video off the desktop at all.
//
// The desktop's list view and multi-select batch actions (Move / Export / Delete)
// have no counterpart here, on purpose. A sortable four-column table is a
// pointer-and-wide-screen affordance, and batch delete/export both end in a
// destination on the machine that holds the bytes — a phone has nowhere to put
// twenty exported files and no undo for a mistaken delete-twenty. The phone's
// Deliverables tab stays what it has always been: browse and open one deliverable.
// Filing and tidying stay desk work.
//
// Folder-as-zip export is absent for the same reason plus a harder one: the
// relay's one shape for bytes is a base64 payload in a single JSON frame
// capped at `MAX_RELAY_BYTES`, and a folder of deliverables is the case that
// cap exists to refuse. A phone also has nowhere useful to put a zip.

import { isBrowsableArchive } from "../../shared-ts/zipDir";
import { ASSET_REQUEST_TIMEOUT_MS, type FleetTransport } from "./transport";
import type { Artifact, ArtifactBlobPayload } from "./types";

export type { Artifact } from "./types";

/**
 * Largest artifact the phone will fetch in a single frame.
 *
 * Mirrors `mobile_relay::MAX_ARTIFACT_FRAME_BYTES`. Kept here as well rather
 * than asked for at runtime so the list can render the "too big to preview"
 * state without a round trip — the server enforces the same number as a
 * backstop. Downloads are not subject to it: they go chunk by chunk.
 */
export const MAX_RELAY_BYTES = 16 * 1024 * 1024;

/** Whether this artifact fits in one frame — i.e. whether it can be previewed. */
export function isFetchable(a: Artifact): boolean {
  return a.sizeBytes <= MAX_RELAY_BYTES;
}

/**
 * Which artifacts the phone can actually show something for.
 *
 * Kept separate from the `kind` the desktop uses: the desktop can stream a
 * video and frame a PDF, the phone is fetching whole bytes into memory. Video
 * and audio are listed but not played here — a playable clip would have to be
 * under 16 MiB, which is not the case for anything worth calling a deliverable.
 *
 * The store's single `text` kind is split three ways here for the same reason
 * the desktop splits it: a markdown spec and an html report are ordinary
 * deliverables (the wiki/artifact rule is audience, not format), and showing
 * either as raw source is not a preview. The split reads the mime the store
 * already derived rather than re-sniffing the extension.
 *
 * The Office three are matched on mime for the same reason and with the same
 * strictness as the desktop's `officeMode`: the renderers read OOXML only, so a
 * legacy .doc (binary CFB) or an .odt (a differently-shaped zip) stays on the
 * placeholder rather than being handed to a parser that will throw. The 16 MiB
 * relay ceiling applies first — `isFetchable` runs before any of this.
 */
export type PreviewKind =
  | "image"
  | "zip"
  | "pdf"
  | "markdown"
  | "html"
  | "text"
  | "docx"
  | "xlsx"
  | "pptx"
  | "none";

const OOXML_MIME: Record<string, PreviewKind> = {
  "application/vnd.openxmlformats-officedocument.wordprocessingml.document": "docx",
  "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet": "xlsx",
  "application/vnd.openxmlformats-officedocument.presentationml.presentation": "pptx",
};

export function previewKind(a: Artifact): PreviewKind {
  if (!isFetchable(a)) return "none";
  return previewKindFor(a.kind, a.mime);
}

/**
 * `previewKind` for something that is not an artifact record — a member of a
 * zip, whose kind comes from `zipEntryKind` and whose mime comes from its name.
 *
 * The relay ceiling is not rechecked here: a member of a fetchable archive is
 * by definition already on the phone.
 */
export function previewKindFor(kind: string, mime: string): PreviewKind {
  if (kind === "image") return "image";
  if (kind === "pdf") return "pdf";
  const base = mime.split(";")[0].trim().toLowerCase();
  if (kind === "text") {
    if (base === "text/markdown") return "markdown";
    if (base === "text/html") return "html";
    return "text";
  }
  // A .zip is browsable as a folder (shared-ts/zipDir.ts); tar/gz/7z are not,
  // and stay on the placeholder.
  if (isBrowsableArchive(base)) return "zip";
  return OOXML_MIME[base] ?? "none";
}

/** Whether this preview renders from decoded text rather than a blob URL. */
export function isTextPreview(kind: PreviewKind): boolean {
  return kind === "markdown" || kind === "html" || kind === "text";
}

/** Whether this preview needs the raw bytes as a Blob for a JS renderer. */
export function isOfficePreview(kind: PreviewKind): kind is "docx" | "xlsx" | "pptx" {
  return kind === "docx" || kind === "xlsx" || kind === "pptx";
}

/** Human-readable size. Mirrors the desktop's `formatBytes`. */
export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  return `${v < 10 ? v.toFixed(1) : Math.round(v)} ${units[i]}`;
}

export async function listArtifacts(client: FleetTransport): Promise<Artifact[]> {
  return client.request<Artifact[]>("artifact_list");
}

/** Raw bytes + mime + download filename for one artifact. */
export async function fetchArtifact(
  client: FleetTransport,
  id: string,
): Promise<{ filename: string; mime: string; bytes: Uint8Array }> {
  const { filename, mime, base64 } = await client.request<ArtifactBlobPayload>(
    "artifact_blob",
    { id },
    ASSET_REQUEST_TIMEOUT_MS,
  );
  return { filename, mime, bytes: base64ToBytes(base64) };
}

function base64ToBytes(b64: string): Uint8Array {
  // Safari 18.2+ / Chrome 133+ decode natively, which matters here: the manual
  // loop below is per-character JS on the main thread, and a download walks
  // hundreds of megabytes through it.
  const native = (Uint8Array as unknown as { fromBase64?: (s: string) => Uint8Array }).fromBase64;
  if (native) return native(b64);
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i += 1) out[i] = bin.charCodeAt(i);
  return out;
}

/**
 * Bytes asked for per chunk.
 *
 * Under the host's own `artifacts::MAX_RANGE_CHUNK` (8 MiB) on purpose: that is
 * a ceiling, not a target, and the smaller ask buys two things on a phone —
 * progress that moves often enough to read, and a smaller base64 string alive
 * at any one moment. Each chunk still costs one round trip, so going much below
 * this trades throughput for nothing.
 */
export const DOWNLOAD_CHUNK_BYTES = 4 * 1024 * 1024;

export interface DownloadProgress {
  /** Bytes reassembled so far. */
  received: number;
  /** Total size the host reported. Null until the first chunk lands. */
  total: number | null;
}

/**
 * The whole artifact, walked over the transport a chunk at a time.
 *
 * This is what the download button uses, and it is deliberately not
 * `fetchArtifact`: that one is a single frame and so inherits
 * `MAX_RELAY_BYTES`, which is exactly the rule that made a rendered video
 * undownloadable. Chunking moves the limit off the wire and onto the device —
 * the parts are assembled into a `Blob`, which the browser is free to spill to
 * disk, rather than into one contiguous `Uint8Array`.
 *
 * `onProgress` exists because the honest answer to "why is it still preparing"
 * is a number. Without it the button sits on one indeterminate label for the
 * entire transfer, which is the bug this path was written to fix.
 *
 * `signal` aborts between chunks rather than mid-chunk: the transport's
 * `request` has no cancel, so the in-flight frame still arrives and is dropped.
 * That bounds the wasted work at one chunk instead of the whole file.
 */
export async function downloadArtifact(
  client: FleetTransport,
  id: string,
  opts: { onProgress?: (p: DownloadProgress) => void; signal?: AbortSignal; version?: string } = {},
): Promise<{ filename: string; mime: string; blob: Blob }> {
  const parts: Uint8Array[] = [];
  let received = 0;
  let total: number | null = null;
  let filename = "";
  let mime = "application/octet-stream";

  do {
    if (opts.signal?.aborted) throw new DOMException("aborted", "AbortError");
    const params: Record<string, unknown> = {
      id,
      offset: received,
      length: DOWNLOAD_CHUNK_BYTES,
    };
    if (opts.version) params.version = opts.version;
    const payload = await client.request<ArtifactBlobPayload>(
      "artifact_blob",
      params,
      ASSET_REQUEST_TIMEOUT_MS,
    );
    filename = payload.filename;
    mime = payload.mime;
    const bytes = base64ToBytes(payload.base64);
    // A host that ignores `offset` answers with the whole file. Taking its word
    // and appending would silently double the download, so trust the bytes:
    // what came back is everything, and the loop is over.
    if (payload.totalSize == null) {
      return { filename, mime, blob: new Blob([bytes as BlobPart], { type: mime }) };
    }
    if (bytes.length === 0) {
      throw new Error(`download stalled at ${received} of ${payload.totalSize} bytes`);
    }
    parts.push(bytes);
    received += bytes.length;
    total = payload.totalSize;
    opts.onProgress?.({ received, total });
  } while (total == null || received < total);

  return { filename, mime, blob: new Blob(parts as BlobPart[], { type: mime }) };
}
