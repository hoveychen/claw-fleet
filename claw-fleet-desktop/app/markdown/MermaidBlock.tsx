import { useEffect, useRef, useState } from "react";
import styles from "./MermaidBlock.module.css";
import { repairMermaidContrastInSvg } from "./mermaidContrast";
import { fitDiagramWidth, naturalWidthFromViewBox } from "./mermaidFit";
import { type MermaidMode, mermaidThemeConfig } from "./mermaidTheme";

/** Distinct ids per render — mermaid mounts a scratch node keyed by this. */
let seq = 0;

/** Reads the theme the app stamps on <html> (absent means dark; see App.css). */
function currentTheme(): MermaidMode {
  return document.documentElement.getAttribute("data-theme") === "light"
    ? "light"
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
  const hostRef = useRef<HTMLDivElement>(null);

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
          // mermaid 的内置 default/dark 色板（姜黄 subgraph、淡紫节点）是全应用
          // 唯一不吃 App.css token 的地方，改走 base + 自己的变量表。字体栈也在
          // 那里（量宽和画宽必须解析成同一个栈，见 mermaidTheme 注释）。
          ...mermaidThemeConfig(theme),
          // mermaid runs its own DOMPurify pass at this level, so labels
          // carrying HTML can't smuggle script into the SVG we inject below.
          securityLevel: "strict",
        });
        const { svg } = await mermaid.render(`mermaid-${seq++}`, code);
        if (cancelled) return;
        // 对比度自愈烤进字符串：一张按深色主题硬编码 `style X fill:#4a3728` 的
        // 图，在 light 主题下标签仍是主题色 #333，整块糊成黑砖（见 mermaidContrast）。
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

  // 宽图别缩到读不了：装不下时按下限钉宽，让 .diagram 的 overflow-x 接管。
  // 挂 ResizeObserver 是因为分屏/侧栏折叠会改容器宽，一次性量完就过期了。
  useEffect(() => {
    const host = hostRef.current;
    if (!host || svg === null) return;
    const apply = () => {
      const el = host.querySelector("svg");
      if (!el) return;
      const natural = naturalWidthFromViewBox(el.getAttribute("viewBox"));
      const pinned = fitDiagramWidth(natural, host.clientWidth);
      if (pinned === null) {
        el.style.removeProperty("width");
        el.style.removeProperty("max-width");
        return;
      }
      // max-width 必须一起写死：mermaid 把自然宽作为**内联** max-width 写在
      // svg 上，样式表里的 .diagram svg{max-width:100%} 根本压不过它。
      el.style.width = `${pinned}px`;
      el.style.maxWidth = "none";
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
