import type { Components } from "react-markdown";
import { MermaidBlock } from "./MermaidBlock";
import { isMermaidPre } from "./mermaidPre";

/**
 * 让一个 markdown 渲染面认得 ```mermaid fence：把它换成渲染好的图，并脱掉
 * react-markdown 给 fence 生成的外层 `<pre>`（普通 fence 的 `<pre>` 照留，脱了
 * 空白会被折叠）。
 *
 * 每个渲染面 spread 这一份，不要再各写各的 —— 之前五处各抄一遍 `code` 覆写，
 * 结果决策/计划 tab、工具详情、Fleet 工具结果三处漏了，同一张图桌面出图、手机
 * 上是一块原始代码。
 */
export const mermaidMarkdownComponents: Components = {
  code: ({ className, children, ...rest }) =>
    /(^|\s)language-mermaid(\s|$)/.test(className ?? "") ? (
      <MermaidBlock code={String(children).replace(/\n$/, "")} />
    ) : (
      <code className={className} {...rest}>
        {children}
      </code>
    ),
  pre: ({ node, children, ...rest }) =>
    isMermaidPre(node) ? <>{children}</> : <pre {...rest}>{children}</pre>,
};
