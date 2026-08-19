import { useEffect, useState } from "react";
import styles from "./MermaidBlock.module.css";
import { repairMermaidContrastInSvg } from "./mermaidContrast";

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
          // 不能用 "inherit"：mermaid 量文字宽度时把图挂在 document.body 下
          // （那里是 sans），而渲染出来的图可能落在 markdown 的 <pre> 里继承到
          // 等宽字体 —— 量出来 92px 的标签实际画 116px，直接被节点框切掉。
          // 给一个两处都解析成同一个栈的 CSS 变量，量和画就对得上了。
          fontFamily: "var(--font-sans)",
        });
        const { svg } = await mermaid.render(`mermaid-${seq++}`, code);
        if (cancelled) return;
        // 对比度自愈烤进字符串：按深色主题硬编码 `style X fill:#4a3728` 的图，
        // 在 light 主题下标签仍是主题色 #333，整块糊成黑砖（见 mermaidContrast）。
        setSvg(repairMermaidContrastInSvg(svg));
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
