import { useEffect, useRef, useState } from "react";
import styles from "./MermaidBlock.module.css";
import { repairMermaidContrastInSvg } from "./mermaidContrast";
import { applyDiagramWidth } from "./mermaidFit";
import { type MermaidMode, mermaidThemeConfig } from "./mermaidTheme";

let seq = 0;

/** theme.ts always stamps html[data-theme]; absent means dark. */
function currentTheme(): MermaidMode {
  return document.documentElement.getAttribute("data-theme") === "light"
    ? "light"
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
  const hostRef = useRef<HTMLDivElement>(null);

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
          // mermaid 的内置 default/dark 色板（姜黄 subgraph、淡紫节点）不吃
          // index.css 的 token，改走 base + 自己的变量表。字体栈也在那里
          // （量宽和画宽必须解析成同一个栈，见 mermaidTheme 注释）。
          ...mermaidThemeConfig(theme),
          securityLevel: "strict",
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

  // 宽图别缩到读不了：装不下时按下限钉宽，让 .diagram 的 overflow-x 接管。
  // 挂 ResizeObserver 是因为转屏/侧栏折叠会改容器宽，一次性量完就过期了。
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

  return (
    <div
      ref={hostRef}
      className={styles.diagram}
      // Trusted: mermaid's own strict-mode renderer sanitized this, not the model.
      dangerouslySetInnerHTML={svg ? { __html: svg } : undefined}
    />
  );
}
