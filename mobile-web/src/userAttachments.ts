// User-direction attachments: files the user hands the agent from a composer or
// a decision answer. The desktop counterpart is claw-fleet-desktop/app/
// userAttachments.ts — the path shapes below must stay in step with it, since
// both sides read back the *same* transcripts.
//
// The one deliberate divergence: the desktop turns a stored path into a
// `fleet-attachment://` URL its webview can load directly. Mobile has no custom
// protocol, so a path becomes a `{key, name}` pair and the bytes come back
// base64-framed through the relay's `user_attachment` method (see
// claw-fleet-core/src/mobile_relay.rs).

import { ASSET_REQUEST_TIMEOUT_MS, type FleetTransport } from "./transport";

/**
 * `~/.fleet/user-attachments/<key>/<name>` — the shape the store returns.
 * Matched against the raw path because that path is all history has: it was
 * frozen into the transcript (`Context files:`) or the decision record
 * (`@<path>`) at send time, and the home dir it sits under may be a desktop
 * home this phone never sees.
 *
 * Separators are `[\\/]`: a Windows desktop freezes a backslash path into the
 * transcript, and the phone reading it back has no idea which OS wrote it.
 */
const STORE_RE = /(?:^|[\\/])\.fleet[\\/]user-attachments[\\/]([^\\/]+)[\\/]([^\\/]+)$/;

/**
 * Pastes from before the store existed were staged in `$TMPDIR/fleet-pasted/`,
 * and it is *that* path the transcript froze. The desktop serves them under a
 * reserved key while the files last. Without this, every history predating the
 * store would show a filename where the screenshot is.
 */
const LEGACY_PASTED_RE = /(?:^|[\\/])fleet-pasted[\\/]([^\\/]+)$/;
const LEGACY_PASTED_KEY = "_pasted";

/** How the relay's `user_attachment` should find an attachment: by store
 *  coordinates, or — for a file the user picked, which keeps its own path and
 *  never enters the store — by that host path. A path is what history froze,
 *  so every render site starts from one and resolves it here. */
export type AttachmentRef = { key: string; name: string } | { path: string };

/**
 * Store coordinates for `path`, or null when the path is neither in the store
 * nor a legacy paste. Callers that want to preview a picked image fall back to
 * `{ path }` (see `imageAttachmentRef`).
 */
export function attachmentRef(path: string): { key: string; name: string } | null {
  const stored = path.match(STORE_RE);
  if (stored) return { key: stored[1], name: stored[2] };
  const legacy = path.match(LEGACY_PASTED_RE);
  if (legacy) return { key: LEGACY_PASTED_KEY, name: legacy[1] };
  return null;
}

/** A host path the relay can resolve on the desktop: POSIX-absolute,
 *  home-relative, or a Windows drive path. */
const HOST_ABS_RE = /^(?:\/|~\/|[A-Za-z]:[\\/])/;

/**
 * How to fetch `path` as an image thumbnail, or null when it is not an image.
 * A picked file (outside the store) is fetched by its host path — the desktop
 * shows it as a picture, so replaying it here as a bare chip read as the image
 * having been lost. The desktop only serves image files for that form.
 */
export function imageAttachmentRef(path: string): AttachmentRef | null {
  if (!isRenderableImage(attachmentName(path))) return null;
  const stored = attachmentRef(path);
  if (stored) return stored;
  return HOST_ABS_RE.test(path) ? { path } : null;
}

/** True when the name looks like an image we can show inline. */
export function isRenderableImage(name: string): boolean {
  return /\.(png|jpe?g|gif|webp|bmp|svg|avif)$/i.test(name);
}

/** Filename component of a path, for labelling a thumbnail. */
export function attachmentName(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

/**
 * The composers append attachments to the prompt as a trailing block:
 *
 *   \n\nContext files:\n- /abs/one.png\n- /abs/two.pdf
 *
 * Claude Code stores that verbatim in the transcript, so it is all history has
 * to work with. Peel it back off the user's actual message: the paths become
 * thumbnails, and the prose is shown without a wall of absolute paths stapled
 * to the end of it.
 *
 * Matching is anchored to the exact shape we emit, so a user who merely *types*
 * the words "Context files:" mid-sentence is left alone.
 */
const CONTEXT_FILES_RE = /\n\nContext files:\n((?:- .+(?:\n|$))+)$/;

export interface SplitContextFiles {
  /** The message with the trailing block removed. */
  body: string;
  /** Absolute paths named by the block, in order. */
  paths: string[];
}

export function splitContextFiles(text: string): SplitContextFiles {
  const m = text.match(CONTEXT_FILES_RE);
  if (!m) return { body: text, paths: [] };
  const paths = m[1]
    .split("\n")
    .map((line) => line.replace(/^- /, "").trim())
    .filter(Boolean);
  return { body: text.slice(0, m.index), paths };
}

export interface SplitAnswer {
  /** The option label / free text, with the mentions removed. */
  core: string;
  /** Paths named by the `@` mentions, in order. */
  attachments: string[];
}

/**
 * Split the `@/path` / `@~/path` mention suffixes a `fleet__ask` answer carries
 * from the label or free text in front of them — the desktop's `splitAttachments`
 * in decisionText.ts, which the history view there feeds to its `AttachmentRow`.
 *
 * Without this the mobile decision history printed the raw answer, so an answer
 * with a picture attached read as `Okay @/Users/…/.fleet/user-attachments/…png`.
 */
export function splitAnswerAttachments(raw: string): SplitAnswer {
  const attachments: string[] = [];
  const kept: string[] = [];
  for (const tok of raw.trim().split(/\s+/)) {
    if (tok.startsWith("@/") || tok.startsWith("@~")) attachments.push(tok.slice(1));
    else kept.push(tok);
  }
  return { core: kept.join(" "), attachments };
}

export interface AttachmentImage {
  mime: string;
  base64: string;
}

/**
 * Fetch one attachment image through the relay: the server-side thumbnail by
 * default, the stored bytes when `full` (what tapping a thumbnail enlarges).
 *
 * Uses the generous asset timeout rather than the 15s control-message default,
 * for the same reason `fetchDecisionAsset` does: a full-size image is MB-scale
 * over a phone link, and a spurious abort strands the <img> forever.
 */
export function fetchAttachmentImage(
  client: FleetTransport,
  ref: AttachmentRef,
  full = false,
): Promise<AttachmentImage> {
  const addr = "path" in ref ? { path: ref.path } : { key: ref.key, name: ref.name };
  return client.request<AttachmentImage>(
    "user_attachment",
    { ...addr, full },
    ASSET_REQUEST_TIMEOUT_MS,
  );
}

/** `data:` URI for a fetched attachment image, ready for an `<img src>`. */
export function attachmentDataUrl(img: AttachmentImage): string {
  return `data:${img.mime};base64,${img.base64}`;
}
