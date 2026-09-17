import type { Components } from "react-markdown";
import { MermaidBlock } from "./MermaidBlock";
import { isMermaidPre } from "./mermaidPre";

/**
 * Make a markdown renderer recognize ```mermaid fences: replace them with
 * rendered diagrams and strip the outer `<pre>` that react-markdown generates
 * for fences (ordinary fences keep their `<pre>`; stripping would collapse whitespace).
 *
 * Every renderer should spread this — don't rewrite it in each one. Previously
 * five places each copied `code` overrides, but decision/plan tabs, tool details,
 * and Fleet tool results missed it. Result: same diagram rendered on desktop,
 * raw code block on mobile.
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
