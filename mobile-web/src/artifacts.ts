// Artifact-store client for the mobile web app. Deliverables are stored on the
// desktop into ~/.fleet/artifacts and read over the relay via `artifact_list` /
// `artifact_blob` (see claw-fleet-core/src/mobile_relay.rs).
//
// The phone deliberately handles only the small half. The relay's one shape for
// bytes is a base64 payload inside a single JSON frame, and base64 adds a third
// on top — there is no honest way to push a rendered video through it. So
// anything over `MAX_RELAY_BYTES` shows its card and points at the desktop's
// export instead of pretending it can fetch it.

import { ASSET_REQUEST_TIMEOUT_MS, type FleetTransport } from "./transport";
import type { Artifact, ArtifactBlobPayload } from "./types";

export type { Artifact } from "./types";

/**
 * Largest artifact the phone will fetch through the relay.
 *
 * Mirrors `mobile_relay::MAX_ARTIFACT_FRAME_BYTES`. Kept here as well rather
 * than asked for at runtime so the list can render the "too big" state without
 * a round trip — the server enforces the same number as a backstop.
 */
export const MAX_RELAY_BYTES = 16 * 1024 * 1024;

/** Whether this artifact's bytes can cross the relay at all. */
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
 */
export function previewKind(
  a: Artifact,
): "image" | "pdf" | "markdown" | "html" | "text" | "none" {
  if (!isFetchable(a)) return "none";
  if (a.kind === "image") return "image";
  if (a.kind === "pdf") return "pdf";
  if (a.kind === "text") {
    const base = a.mime.split(";")[0].trim().toLowerCase();
    if (base === "text/markdown") return "markdown";
    if (base === "text/html") return "html";
    return "text";
  }
  return "none";
}

/** Whether this preview renders from decoded text rather than a blob URL. */
export function isTextPreview(kind: ReturnType<typeof previewKind>): boolean {
  return kind === "markdown" || kind === "html" || kind === "text";
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
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i += 1) out[i] = bin.charCodeAt(i);
  return out;
}
