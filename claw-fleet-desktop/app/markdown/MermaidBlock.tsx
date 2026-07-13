import { useEffect, useRef, useState } from "react";
import styles from "./MermaidBlock.module.css";

/** Distinct ids per render — mermaid mounts a scratch node keyed by this. */
let seq = 0;

/** Reads the theme the app stamps on <html> (absent means dark; see App.css). */
function currentTheme(): "dark" | "default" {
  return document.documentElement.getAttribute("data-theme") === "light"
    ? "default"
    : "dark";
}

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
  const [theme, setTheme] = useState(currentTheme);
  const holder = useRef<HTMLDivElement>(null);

  // Re-render on theme flips so a diagram doesn't stay dark-on-paper.
  useEffect(() => {
    const obs = new MutationObserver(() => setTheme(currentTheme()));
    obs.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-theme"],
    });
    return () => obs.disconnect();
  }, []);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const mermaid = (await import("mermaid")).default;
        mermaid.initialize({
          startOnLoad: false,
          theme,
          // mermaid runs its own DOMPurify pass at this level, so labels
          // carrying HTML can't smuggle script into the SVG we inject below.
          securityLevel: "strict",
          fontFamily: "inherit",
        });
        const { svg } = await mermaid.render(`mermaid-${seq++}`, code);
        if (cancelled) return;
        setSvg(svg);
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
      ref={holder}
      className={styles.diagram}
      // Trusted: the SVG string comes straight out of mermaid's own sanitizing
      // renderer (securityLevel "strict"), not from the model.
      dangerouslySetInnerHTML={svg ? { __html: svg } : undefined}
    />
  );
}
