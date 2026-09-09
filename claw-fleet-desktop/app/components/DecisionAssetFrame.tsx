import { useEffect, useState } from "react";
import type { CSSProperties } from "react";
import { AutoHeightFrame } from "./AutoHeightFrame";
import { decisionAssetUrl } from "../decisionAssets";
import { fetchDecisionAssetDoc } from "../decisionAssetDoc";
import { isWebBuild } from "../hostEnv";

/**
 * A decision card's stored preview (`~/.fleet/decision-assets/<id>/q<idx>/`),
 * rendered the way this host can actually reach it.
 *
 * Desktop: point the sandboxed frame at `fleet-decision://…/index.html` and let
 * the document pull its own images — no gateway sits in front of a custom
 * protocol.
 *
 * Browser build: the frame's own subresource requests do not carry the
 * deployment's session cookie (see `decisionAssetDoc.ts` for the measurement),
 * so the parent fetches the document and its images and hands the frame one
 * self-contained `srcDoc`. If that fetch fails we fall back to the plain `src`,
 * which is exactly today's behaviour — an ungated deployment keeps working even
 * if the inlining path breaks.
 *
 * Used by all three places a stored preview appears — the live card, the
 * history list and the transcript's inline card — so the host fork lives once.
 */
export function DecisionAssetFrame({
  id,
  qidx,
  theme,
  title,
  className,
  style,
  minHeight,
}: {
  id: string;
  qidx: string;
  theme?: "dark" | "light";
  title: string;
  className?: string;
  style?: CSSProperties;
  minHeight?: number;
}) {
  const [doc, setDoc] = useState<string | null>(null);
  const [inlineFailed, setInlineFailed] = useState(false);

  useEffect(() => {
    if (!isWebBuild()) return;
    let cancelled = false;
    setDoc(null);
    setInlineFailed(false);
    fetchDecisionAssetDoc(id, qidx, theme)
      .then((html) => {
        if (!cancelled) setDoc(html);
      })
      .catch((err) => {
        console.warn(`[decision-asset] ${id}/${qidx} inline failed, using direct src:`, err);
        if (!cancelled) setInlineFailed(true);
      });
    return () => {
      cancelled = true;
    };
  }, [id, qidx, theme]);

  const useSrc = !isWebBuild() || inlineFailed;
  return (
    <AutoHeightFrame
      title={title}
      className={className}
      style={style}
      minHeight={minHeight}
      src={useSrc ? decisionAssetUrl(id, qidx, "index.html", theme) : undefined}
      // Empty until the fetch lands: rendering the direct `src` first would
      // flash the broken-image glyph this component exists to remove.
      srcDoc={useSrc ? undefined : (doc ?? "")}
    />
  );
}
