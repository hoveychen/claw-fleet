import { useEffect, useRef, useState } from "react";
import styles from "./MermaidBlock.module.css";
import { repairMermaidLabelContrast } from "./mermaidContrast";

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
          // 不能用 "inherit"：mermaid 量文字宽度时把图挂在 document.body 下
          // （那里是 sans），而渲染出来的图可能落在 markdown 的 <pre> 里继承到
          // 等宽字体 —— 量出来 92px 的标签实际画 116px，直接被节点框切掉。
          // 给一个两处都解析成同一个栈的 CSS 变量，量和画就对得上了。
          fontFamily: "var(--font-sans)",
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

  // 图注进 DOM 之后再补对比度：一张按深色主题硬编码 `style X fill:#4a3728`
  // 的图，在 light 主题下标签仍是主题色 #333，整块糊成黑砖（见 mermaidContrast）。
  useEffect(() => {
    if (svg !== null && holder.current) {
      repairMermaidLabelContrast(holder.current);
    }
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
      ref={holder}
      className={styles.diagram}
      // Trusted: the SVG string comes straight out of mermaid's own sanitizing
      // renderer (securityLevel "strict"), not from the model.
      dangerouslySetInnerHTML={svg ? { __html: svg } : undefined}
    />
  );
}
