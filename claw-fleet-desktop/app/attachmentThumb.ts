/**
 * A thumbnail for a staged composer attachment, for the paths that have no
 * `previewUrl`.
 *
 * Only one of the five ways to add an attachment ever produced a preview: a
 * paste, which arrives as a `File` and can be turned into an object URL on the
 * spot (`ChatComposer`'s `handlePaste`). The other four — the ＋ menu's picker,
 * the desktop's OS drag-drop, the browser build's HTML5 drop, and a decision
 * card's file pick — hand the composer nothing but `{path, name}`, so an image
 * the user picked showed as a bare filename chip while a pasted one showed as a
 * picture. That asymmetry reads as "the cloud lost my image" because in the
 * browser build picking is the only practical way in.
 *
 * Two sources, cheapest first:
 *
 * 1. The attachment store. The browser build has already uploaded a picked file
 *    into `~/.fleet/user-attachments/` (it has no host path to hand the agent
 *    otherwise), so its path is a store path and `userAttachmentUrl` resolves
 *    it to a URL the webview or the tab can load directly — no bytes through
 *    the transport.
 * 2. `read_external_file`, for any other host path — what the desktop's picker
 *    returns, since a file the user picked keeps its own path there. This is
 *    the same door `markdown/localImages` uses for agent-written image refs:
 *    implemented on both transports, capped at `IMAGE_PREVIEW_CAP`, and it
 *    works when the path is on a remote host that no `file://` URL could reach.
 *
 * Failure stays quiet, unlike the markdown case: there the image *is* the
 * content and a blank is the bug, whereas here the chip already names the file.
 * `useAttachmentThumbState` still reports it, so a caller can mark the chip.
 */

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { isRenderableImage, userAttachmentUrl } from "./userAttachments";
import type { ExplorerFileContent } from "./components/ExplorerPane";

export interface ThumbSource {
  path: string;
  name: string;
  /** Already resolved by the caller (a paste's object URL) — wins outright. */
  previewUrl?: string;
}

/**
 * The store URL for an attachment, or null when the path is not in the store
 * or the name is not an image we can render. Split out from the hook so the
 * decision it encodes is testable without a renderer.
 */
export function storeThumbUrl(a: ThumbSource): string | null {
  if (!isRenderableImage(a.name)) return null;
  return userAttachmentUrl(a.path);
}

/** What `useAttachmentThumbState` knows about an attachment's thumbnail. */
export interface AttachmentThumbState {
  /** The thumbnail URL, or null when there is none (yet). */
  src: string | null;
  /** A `read_external_file` read is in flight — a thumbnail may still arrive. */
  pending: boolean;
  /** The name says image, but the read failed or came back as something other
   *  than an image. Lets a caller show a "broken image" chip rather than a
   *  plain file chip. */
  failed: boolean;
}

/** `src` for an attachment chip's thumbnail, or null when there is none. */
export function useAttachmentThumb(a: ThumbSource): string | null {
  return useAttachmentThumbState(a).src;
}

/**
 * The thumbnail plus its load state. `pending` ends on success *and* on
 * failure, so a skeleton keyed on it can never stay up for good.
 */
export function useAttachmentThumbState(a: ThumbSource): AttachmentThumbState {
  const { path, name, previewUrl } = a;
  const direct = previewUrl ?? storeThumbUrl({ path, name });
  const needsRead = !direct && isRenderableImage(name);
  // The settled read, tagged with the path it was for so a chip handed a new
  // path reads as pending again instead of showing the previous answer.
  const [read, setRead] = useState<{ path: string; src: string | null } | null>(null);

  useEffect(() => {
    // Nothing to read when the caller already has a URL, and nothing worth
    // reading when the name says it is not an image — `read_external_file`
    // would haul a 40 MiB zip across the transport to answer "binary".
    if (!needsRead) {
      setRead(null);
      return;
    }
    let live = true;
    setRead(null);
    invoke<ExplorerFileContent>("read_external_file", { path })
      .then((content) => {
        if (!live) return;
        setRead({
          path,
          src: content.kind === "image" ? `data:${content.mime};base64,${content.base64}` : null,
        });
      })
      .catch(() => {
        // The chip still names the file; the caller decides how to mark it.
        if (live) setRead({ path, src: null });
      });
    return () => {
      live = false;
    };
  }, [needsRead, path]);

  if (direct) return { src: direct, pending: false, failed: false };
  if (!needsRead) return { src: null, pending: false, failed: false };
  const settled = read !== null && read.path === path;
  return {
    src: settled ? read.src : null,
    pending: !settled,
    failed: settled && read.src === null,
  };
}
