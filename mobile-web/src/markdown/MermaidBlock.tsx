import { useEffect, useState } from "react";
import styles from "./MermaidBlock.module.css";

let seq = 0;

/** theme.ts always stamps html[data-theme]; absent means dark. */
function currentTheme(): "dark" | "default" {
  return document.documentElement.getAttribute("data-theme") === "light"
    ? "default"
    : "dark";
}

/**
 * A ```mermaid fence rendered to SVG. mermaid is ~1MB, so it loads lazily on
 * first use — a phone that never opens a diagram never pays for it. A diagram
 * that fails to parse degrades to its source rather than blanking the message.
 */
export function MermaidBlock({ code }: { code: string }) {
  const [svg, setSvg] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [theme, setTheme] = useState(currentTheme);

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
          securityLevel: "strict",
          fontFamily: "inherit",
        });
        const { svg } = await mermaid.render(`mermaid-${seq++}`, code);
        if (cancelled) return;
        setSvg(svg);
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

  if (error !== null) {
    return (
      <div className={styles.failed}>
        <div className={styles.failed_label}>mermaid 渲染失败</div>
        <pre className={styles.failed_error}>{error}</pre>
        <pre className={styles.failed_source}>{code}</pre>
      </div>
    );
  }

  return (
    <div
      className={styles.diagram}
      // Trusted: mermaid's own strict-mode renderer sanitized this, not the model.
      dangerouslySetInnerHTML={svg ? { __html: svg } : undefined}
    />
  );
}
