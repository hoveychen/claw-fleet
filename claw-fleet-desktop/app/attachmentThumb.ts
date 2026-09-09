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
 * Failure is silent on purpose, unlike the markdown case: there the image *is*
 * the content and a blank is the bug, whereas here the chip already names the
 * file and a missing thumbnail costs nothing.
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

/** `src` for an attachment chip's thumbnail, or null when there is none. */
export function useAttachmentThumb(a: ThumbSource): string | null {
  const { path, name, previewUrl } = a;
  const direct = previewUrl ?? storeThumbUrl({ path, name });
  const [read, setRead] = useState<string | null>(null);

  useEffect(() => {
    // Nothing to read when the caller already has a URL, and nothing worth
    // reading when the name says it is not an image — `read_external_file`
    // would haul a 40 MiB zip across the transport to answer "binary".
    if (direct || !isRenderableImage(name)) {
      setRead(null);
      return;
    }
    let live = true;
    setRead(null);
    invoke<ExplorerFileContent>("read_external_file", { path })
      .then((content) => {
        if (live && content.kind === "image") {
          setRead(`data:${content.mime};base64,${content.base64}`);
        }
      })
      .catch(() => {
        // A chip with no thumbnail, which is what it looked like anyway.
      });
    return () => {
      live = false;
    };
  }, [direct, path, name]);

  return direct ?? read;
}
