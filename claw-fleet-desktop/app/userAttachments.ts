/**
 * User-direction attachments: files the user hands the agent, from a composer
 * paste or a decision-panel pick.
 *
 * Two things live here because both sides of the attachment's life need them:
 * the composers, which resolve a staged file into the path that goes into the
 * prompt, and the history views, which turn that path back into something
 * renderable.
 *
 * The counterpart for the agent → user direction is `decisionAssets.ts`.
 */

import { invoke } from "@tauri-apps/api/core";
import type { ChatComposerAttachment, ChatComposerStagedAttachment } from "./components/ChatComposer";

/**
 * Hand a staged attachment to the backend and take back the path the agent will
 * actually see.
 *
 * This has to go through the Backend rather than using the staged path directly:
 * under RemoteBackend the agent runs on the probe host and cannot read anything
 * on the desktop's disk. `fromClipboard` additionally decides whether the bytes
 * are ingested into the persistent store (pasted bytes, which have no home) or
 * left where they are (a file the user picked, whose path already means
 * something).
 */
export async function resolveStagedAttachment(
  staged: ChatComposerStagedAttachment,
): Promise<ChatComposerAttachment> {
  const resolvedPath = await invoke<string>("upload_elicitation_attachment", {
    sourcePath: staged.path,
    fromClipboard: staged.fromClipboard,
  });
  return {
    path: resolvedPath,
    name: staged.name,
    fromClipboard: staged.fromClipboard,
    previewUrl: staged.preview?.previewUrl,
    width: staged.preview?.width,
    height: staged.preview?.height,
  };
}

/**
 * `~/.fleet/user-attachments/<key>/<name>` — the shape `ingest` returns. Matched
 * against the raw path because that path is all history has: it was frozen into
 * the transcript (`Context files:`) or the decision record (`@<path>`) at send
 * time, and the home dir it sits under may be a *remote* home we never see.
 */
const STORE_RE = /(?:^|\/)\.fleet\/user-attachments\/([^/]+)\/([^/]+)$/;

/** True when `path` names a file in the persistent user-attachment store. */
export function isStoredAttachment(path: string): boolean {
  return STORE_RE.test(path);
}

/**
 * URL of a stored attachment through the `fleet-attachment` custom protocol, or
 * null when the path isn't in the store (a file the user picked keeps its own
 * path — we have no license to read arbitrary paths off the agent host).
 *
 * Built by hand rather than with `convertFileSrc` for the same reason as
 * `decisionAssetUrl`: the desktop may be pointed at a remote backend, where the
 * file is on the probe host and no local file:// URL could reach it.
 */
export function userAttachmentUrl(path: string): string | null {
  const m = path.match(STORE_RE);
  if (!m) return null;
  const seg = [m[1], m[2]].map(encodeURIComponent).join("/");
  return navigator.userAgent.includes("Windows")
    ? `http://fleet-attachment.localhost/${seg}`
    : `fleet-attachment://localhost/${seg}`;
}

/** True when the name looks like an image we can show inline. */
export function isRenderableImage(name: string): boolean {
  return /\.(png|jpe?g|gif|webp|bmp|svg|avif)$/i.test(name);
}

/** Filename component of a path, for labelling a thumbnail. */
export function attachmentName(path: string): string {
  const parts = path.split("/");
  return parts[parts.length - 1] || path;
}

/**
 * The composers append attachments to the prompt as a trailing block:
 *
 *   \n\nContext files:\n- /abs/one.png\n- /abs/two.pdf
 *
 * Claude Code stores that verbatim in the transcript, so it is all history has
 * to work with. Peel it back off the user's actual message: the paths become
 * attachment chips, and the prose is shown without a wall of absolute paths
 * stapled to the end of it.
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
