import type { ImageBlock } from "./types";

/**
 * `data:` URI for an inline base64 image block, or null when the block carries
 * no usable payload.
 *
 * Both places images appear in a transcript — a user's pasted screenshot and a
 * `Read` of an image file — carry the identical shape
 * `{type: "image", source: {type: "base64", media_type, data}}`.
 */
export function imageDataUrl(block: ImageBlock): string | null {
  const source = block.source;
  if (!source || source.type !== "base64" || !source.data) return null;
  return `data:${source.media_type || "image/png"};base64,${source.data}`;
}
