import { useEffect, useRef, useState } from "react";
import type { CSSProperties } from "react";
import {
  FRAME_MIN_HEIGHT,
  parseFrameHeight,
  shouldApplyFrameHeight,
} from "../decisionFrame";

/**
 * The iframe every decision card renders its `html` preview in — the one place
 * that knows how a fleet__ask preview is sandboxed and sized.
 *
 * `sandbox="allow-scripts"` **without** `allow-same-origin`: the document keeps
 * an opaque origin, so agent-authored HTML still cannot touch the app's DOM,
 * storage, cookies or Tauri IPC. The one capability it gains is `postMessage`,
 * which is exactly what it needs to report its own height — an iframe otherwise
 * has the 300x150 intrinsic size of a replaced element and never grows to fit its
 * content, so a taller preview gets squashed into an inner scroll box. Adding
 * `allow-same-origin` alongside `allow-scripts` would let the document escape the
 * sandbox entirely; never do that here.
 *
 * The child (script injected by `mcp_ipc::AUTOHEIGHT_SCRIPT`) posts its height;
 * we identify it by `e.source` rather than `e.origin`, which is the string "null"
 * for an opaque origin and so cannot distinguish one card's frame from another's.
 */
export function AutoHeightFrame({
  title,
  src,
  srcDoc,
  className,
  style,
  minHeight = FRAME_MIN_HEIGHT,
}: {
  title: string;
  src?: string;
  srcDoc?: string;
  className?: string;
  style?: CSSProperties;
  minHeight?: number;
}) {
  const ref = useRef<HTMLIFrameElement>(null);
  const [height, setHeight] = useState<number | null>(null);

  useEffect(() => {
    // A different document is about to load: drop the old measurement rather
    // than briefly rendering the new one at the previous card's height.
    setHeight(null);
    const onMessage = (e: MessageEvent) => {
      if (!ref.current || e.source !== ref.current.contentWindow) return;
      const h = parseFrameHeight(e.data);
      if (h === null) return;
      setHeight((cur) => (shouldApplyFrameHeight(cur, h) ? h : cur));
    };
    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  }, [src, srcDoc]);

  return (
    <iframe
      ref={ref}
      title={title}
      sandbox="allow-scripts"
      src={src}
      srcDoc={srcDoc}
      className={className}
      style={{
        ...style,
        // Until the first message lands (old cards whose stored html predates the
        // script, or a document that never loads) the min-height still applies.
        height: height === null ? undefined : `${height}px`,
        minHeight: `${minHeight}px`,
      }}
    />
  );
}
