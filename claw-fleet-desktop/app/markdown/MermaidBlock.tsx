import { useEffect, useRef, useState } from "react";
import { useDocumentTheme } from "../hooks/useDocumentTheme";
import styles from "./MermaidBlock.module.css";
import { repairMermaidContrastInSvg } from "./mermaidContrast";
import { applyDiagramWidth } from "./mermaidFit";
import { type MermaidMode, mermaidThemeConfig } from "./mermaidTheme";

/** Distinct ids per render — mermaid mounts a scratch node keyed by this. */
let seq = 0;

/**
 * A ```mermaid fenced block, rendered to SVG.
 *
 * mermaid is ~1MB, so it is imported lazily on first use rather than bundled
 * into the entry chunk that every window loads. A diagram whose source doesn't
 * parse degrades to the raw code plus the parser's complaint — a model that
 * emits a slightly-off diagram must not blank out the message carrying it.
 */
export function MermaidBlock({ code }: { code: string }) {
  const [svg, setSvg] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Re-renders on theme flips so a diagram doesn't stay dark-on-paper.
  const theme: MermaidMode = useDocumentTheme();
  const hostRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const mermaid = (await import("mermaid")).default;
        mermaid.initialize({
          startOnLoad: false,
          // mermaid's built-in default/dark color palette (amber subgraph, light
          // purple nodes) is the only place in the app that doesn't consume App.css
          // tokens; switch to base + custom variable table. Font stack lives there
          // too (measured width and drawn width must parse to the same stack; see
          // mermaidTheme comments).
          ...mermaidThemeConfig(theme),
          // mermaid runs its own DOMPurify pass at this level, so labels
          // carrying HTML can't smuggle script into the SVG we inject below.
          securityLevel: "strict",
        });
        const { svg } = await mermaid.render(`mermaid-${seq++}`, code);
        if (cancelled) return;
        // Contrast self-healing is baked into the string: a diagram hard-coded
        // for dark theme with `style X fill:#4a3728` becomes unreadable on light
        // theme where labels stay at theme color #333 (see mermaidContrast).
        setSvg(repairMermaidContrastInSvg(svg));
        setError(null);
      } catch (e) {
        if (cancelled) return;
        setSvg(null);
        setError(e instanceof Error ? e.message : String(e));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [code, theme]);

  // Don't shrink wide diagrams until unreadable: when it doesn't fit, pin width
  // to a minimum and let .diagram's overflow-x take over. ResizeObserver is needed
  // because split screen/sidebar collapse changes container width; one-time
  // measurement expires.
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
        <div className={styles.failed_label}>mermaid 渲染失败：{error}</div>
        <pre className={styles.failed_source}>{code}</pre>
      </div>
    );
  }

  return (
    <div
      ref={hostRef}
      className={styles.diagram}
      // Trusted: the SVG string comes straight out of mermaid's own sanitizing
      // renderer (securityLevel "strict"), not from the model.
      dangerouslySetInnerHTML={svg ? { __html: svg } : undefined}
    />
  );
}
