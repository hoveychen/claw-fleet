import { useEffect, useState } from "react";
import type { ReactNode, RefObject } from "react";
import { parseImgZoom } from "../../../shared-ts/iframeImgZoom";
import { ImageLightbox } from "./ImageLightbox";
import { Presence } from "./Presence";

/**
 * Click-to-zoom for images inside a sandboxed preview frame. The frame's
 * bridge (`mcp_ipc::IMG_ZOOM_SCRIPT`, injected by core) posts the clicked
 * image's src; this opens the app lightbox on it. Messages are matched by
 * `e.source`, like the height reporter, because an opaque origin is "null" for
 * every frame. Returns the lightbox element for the caller to render.
 */
export function useIframeImgZoom(frame: RefObject<HTMLIFrameElement | null>): ReactNode {
  const [src, setSrc] = useState<string | null>(null);

  useEffect(() => {
    const onMessage = (e: MessageEvent) => {
      if (!frame.current || e.source !== frame.current.contentWindow) return;
      const next = parseImgZoom(e.data);
      if (next) setSrc(next);
    };
    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  }, [frame]);

  return (
    <Presence when={src !== null}>
      {src !== null && <ImageLightbox src={src} onClose={() => setSrc(null)} />}
    </Presence>
  );
}
