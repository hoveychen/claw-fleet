// Generated-image fetch (pictures made by fleet__image / fleet__image_edit).
// The phone's equivalent of the desktop's `fleet-genimage://` thumbnail strip;
// kept out of the views so the relay round-trip is unit-testable without a
// React render. Bytes come back base64-framed through the relay's
// `session_images` / `session_image` methods
// (see claw-fleet-core/src/mobile_relay.rs).

import { ASSET_REQUEST_TIMEOUT_MS, type FleetTransport } from "./transport";

export interface SessionImageEntry {
  /** Bare filename, the only handle the phone needs. Absolute paths on the
   *  agent's machine are meaningless here and are not sent. */
  name: string;
  bytes: number;
}

export interface SessionImageBytes {
  mime: string;
  base64: string;
}

/** List the images belonging to one handle (a native `img-<uuid>` handle or a
 *  legacy Codex thread id — the agent side routes on the prefix, this side
 *  does not care which). */
export async function fetchSessionImages(
  client: FleetTransport,
  session: string,
): Promise<SessionImageEntry[]> {
  const res = await client.request<{ images?: SessionImageEntry[] }>(
    "session_images",
    { session },
  );
  return res?.images ?? [];
}

/** Fetch one generated image's bytes.
 *  Uses the generous asset timeout, not the 15s control-message default: a 4K
 *  render is MB-scale even after the agent-side downscale, and a spurious 15s
 *  abort strands the <img> forever — the reply lands after the pending entry
 *  was already dropped and is silently discarded. Same pitfall as
 *  decision_asset. */
export function fetchSessionImage(
  client: FleetTransport,
  session: string,
  name: string,
): Promise<SessionImageBytes> {
  return client.request<SessionImageBytes>(
    "session_image",
    { session, name },
    ASSET_REQUEST_TIMEOUT_MS,
  );
}

/** Where one tail thumbnail came from, so the host can find its original:
 *  the `index`-th `_thumbs` entry of a tool_result, or the top-level `image`
 *  block at content index `block` of the record with that `uuid`. */
export type TranscriptImageAddr =
  | { tool_use_id: string; index: number }
  | { uuid: string; block: number };

/** Full-resolution version of a transcript thumbnail (the tail ships ~256px
 *  JPEGs only), as a data URI for the lightbox. Asset timeout for the same
 *  reason as `fetchSessionImage`. */
export async function fetchTranscriptImage(
  client: FleetTransport,
  path: string,
  addr: TranscriptImageAddr,
): Promise<string> {
  const res = await client.request<SessionImageBytes>(
    "transcript_image",
    { path, ...addr },
    ASSET_REQUEST_TIMEOUT_MS,
  );
  return `data:${res.mime};base64,${res.base64}`;
}
