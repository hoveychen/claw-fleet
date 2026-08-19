/**
 * `pre` 覆写用的判定：这个 `<pre>` 包着的是不是一段 ```mermaid fence。
 *
 * 与桌面 claw-fleet-desktop/app/markdown/mermaidPre.ts 保持同步（两个 app 是独立
 * 的 vite 包，逻辑复制而非共享；单测在桌面那一侧）。
 *
 * 每个 markdown 渲染点都是在 `code` 组件上把 mermaid fence 换成 <MermaidBlock>，
 * 但 react-markdown 给 fence 生成的外层 `<pre>` 还在，于是图被塞进一个等宽字体、
 * 自带背景的块里：字体继承会让 mermaid 量出来的标签宽度对不上（见 MermaidBlock
 * 里的 fontFamily 注释），背景则和图自己的卡片叠成双层框。所以挂载点统一再加一个
 * `pre` 覆写，只对 mermaid fence 脱掉外层，别的 fence 一切照旧。
 */

type HastLike = {
  children?: Array<{
    tagName?: string;
    properties?: { className?: unknown };
  }>;
};

export function isMermaidPre(node: unknown): boolean {
  const first = (node as HastLike | null | undefined)?.children?.[0];
  if (!first || first.tagName !== "code") return false;
  const cls = first.properties?.className;
  const names = Array.isArray(cls)
    ? cls.map(String)
    : typeof cls === "string"
      ? cls.split(/\s+/)
      : [];
  return names.includes("language-mermaid");
}
