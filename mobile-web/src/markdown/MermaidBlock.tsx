import { useEffect, useMemo, useRef, useState } from "react";
import { SkeletonCard } from "../views/loading";
import styles from "./MermaidBlock.module.css";
import { repairMermaidContrastInSvg } from "./mermaidContrast";
import { applyDiagramWidth } from "./mermaidFit";
import { type MermaidMode, mermaidThemeConfig } from "./mermaidTheme";

let seq = 0;

/**
 * Finished SVGs keyed by theme + source, so a remount paints synchronously
 * instead of flashing the skeleton (and shifting the prose below) while
 * mermaid re-renders. Same fix as the desktop MermaidBlock.
 */
const svgCache = new Map<string, string>();
const SVG_CACHE_MAX = 64;

function cacheKey(theme: MermaidMode, code: string): string {
  return `${theme}\u0000${code}`;
}

function rememberSvg(key: string, svg: string): void {
  svgCache.delete(key);
  svgCache.set(key, svg);
  if (svgCache.size > SVG_CACHE_MAX) {
    const oldest = svgCache.keys().next().value;
    if (oldest !== undefined) svgCache.delete(oldest);
  }
}

/** `theme.ts` always stamps `html[data-theme]`; if absent, dark mode is assumed. */
function currentTheme(): MermaidMode {
  return document.documentElement.getAttribute("data-theme") === "light"
    ? "light"
    : "dark";
}

/**
 * A ```mermaid code fence rendered to SVG. Mermaid is ~1MB, so it loads
 * lazily on first use — a phone that never opens a diagram never pays for it.
 * A diagram that fails to parse shows its source instead of blanking the message.
 */
export function MermaidBlock({ code }: { code: string }) {
  const [theme, setTheme] = useState(currentTheme);
  const [svg, setSvg] = useState<string | null>(
    () => svgCache.get(cacheKey(theme, code)) ?? null,
  );
  const [error, setError] = useState<string | null>(null);
  const hostRef = useRef<HTMLDivElement>(null);
  // One object per SVG string: React 19 rewrites innerHTML whenever this prop
  // object is new, which swapped in a fresh <svg> on every parent re-render and
  // dropped the width applyDiagramWidth had pinned on the old one.
  const html = useMemo(() => (svg === null ? null : { __html: svg }), [svg]);

  useEffect(() => {
    const obs = new MutationObserver(() => setTheme(currentTheme()));
    obs.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-theme"],
    });
    return () => obs.disconnect();
  }, []);

  useEffect(() => {
    const key = cacheKey(theme, code);
    const cached = svgCache.get(key);
    if (cached !== undefined) {
      setSvg(cached);
      setError(null);
      return;
    }
    let cancelled = false;
    (async () => {
      try {
        const mermaid = (await import("mermaid")).default;
        mermaid.initialize({
          startOnLoad: false,
          // Mermaid's built-in default/dark palettes (ginger subgraphs, pale nodes)
          // do not respect index.css tokens; switch to base + custom variables.
          // Font stack is also there (measured and drawn widths must parse the
          // same stack — see mermaidTheme comment).
          ...mermaidThemeConfig(theme),
          securityLevel: "strict",
        });
        const { svg } = await mermaid.render(`mermaid-${seq++}`, code);
        if (cancelled) return;
        // Contrast self-healing baked into the SVG string: diagrams hard-coded with
        // dark-mode `style X fill:#4a3728` would show labels in theme color #333 under
        // light mode, resulting in dark text on dark background (see mermaidContrast).
        const repaired = repairMermaidContrastInSvg(svg);
        rememberSvg(key, repaired);
        setSvg(repaired);
        setError(null);
      } catch (e) {
        if (cancelled) return;
        const msg = e instanceof Error ? e.message : String(e);
        // Surface the real cause: this same fence renders fine in Chromium, so
        // a failure here is environment-specific (chunk load / MIME, engine
        // parse, or a render-time DOM gap) and the message tells them apart.
        console.error("mermaid render failed:", e);
        setSvg(null);
        setError(msg);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [code, theme]);

  // Wide diagrams should not shrink below readability: if it does not fit,
  // pin the width to a minimum and let .diagram's overflow-x take over.
  // ResizeObserver is needed because screen rotation/sidebar collapse changes
  // container width, so a one-time measurement becomes stale.
  useEffect(() => {
    const host = hostRef.current;
    if (!host || svg === null) return;
    const apply = () => {
      const el = host.querySelector("svg");
      if (!el) return;
      applyDiagramWidth(el, host.clientWidth);
    };
    apply();
    const ro = new ResizeObserver(apply);
    ro.observe(host);
    return () => ro.disconnect();
  }, [svg]);

  if (error !== null) {
    return (
      <div className={styles.failed}>
        <div className={styles.failed_label}>mermaid 渲染失败</div>
        <pre className={styles.failed_error}>{error}</pre>
        <pre className={styles.failed_source}>{code}</pre>
      </div>
    );
  }

  // First render: mermaid is still loading / laying out. Hold a diagram-sized
  // block so the text below does not jump when the SVG pops in.
  if (svg === null) {
    return <SkeletonCard className={styles.diagram} height={160} />;
  }

  return (
    <div
      ref={hostRef}
      className={styles.diagram}
      // Trusted: mermaid's own strict-mode renderer sanitized this, not the model.
      dangerouslySetInnerHTML={html ?? undefined}
    />
  );
}
