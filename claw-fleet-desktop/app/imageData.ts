import type { ImageBlock, ToolResultBlock } from "./types";
import { userAttachmentUrl } from "./userAttachments";

/**
 * `src` for an image block — a `data:` URI for an inline one, a
 * `fleet-attachment://` URL for one that names a file in the store — or null
 * when the block carries no usable payload.
 *
 * Two source shapes reach here. Claude Code's places — a user's pasted
 * screenshot, a `Read` of an image file — carry the bytes inline as
 * `{type: "base64", media_type, data}`. A dsh transcript instead carries
 * `{type: "path", media_type, path}`: dsh's log holds only a durable attachment
 * reference, and inlining the bytes would put them through the transport's
 * 4 KiB string trim, which corrupts base64 beyond recovery. The store path goes
 * through the same custom protocol the composer's own attachments render with.
 */
export function imageDataUrl(block: ImageBlock): string | null {
  const source = block.source;
  if (!source) return null;
  if (source.type === "path") {
    return source.path ? userAttachmentUrl(source.path) : null;
  }
  if (source.type !== "base64" || !source.data) return null;
  return `data:${source.media_type || "image/png"};base64,${source.data}`;
}

/** Substring of the marker the Rust transport trim (`message_trim::preview_of`)
 *  appends to every truncated string leaf — including an image source's base64,
 *  which the suffix makes permanently undecodable. */
const FLEET_TRUNCATED_MARKER = "[Fleet truncated ";

/**
 * True when this image block's base64 is a transport-trimmed preview rather
 * than real image data. Such a payload can never decode; callers must show a
 * truncation placeholder (and recover the full body via `get_tool_result_full`)
 * instead of feeding it to an `<img>`. Detected from the marker *inside* the
 * data so it still works when the message-level `_fleetTruncated` flag was
 * lost somewhere along the transport.
 */
export function isTrimmedImageData(block: ImageBlock): boolean {
  const data = block.source?.data;
  return typeof data === "string" && data.includes(FLEET_TRUNCATED_MARKER);
}

/**
 * True when a tool_result carries at least one transport-trimmed image block —
 * the card-level cue to refetch the full result on expand even when the
 * message-level `_fleetTruncated` flag is absent.
 */
export function resultHasTrimmedImage(result?: ToolResultBlock): boolean {
  if (!result || !Array.isArray(result.content)) return false;
  return result.content.some(
    (b) =>
      typeof b === "object" &&
      b !== null &&
      (b as { type?: string }).type === "image" &&
      isTrimmedImageData(b as ImageBlock),
  );
}
