// 一份 markdown 正文的组件表，所有渲染面共用。
//
// 分两个来源：mermaid fence 出图（mermaidComponents）和链接可点（linkComponents）。
// 之所以合成一份而不是让每个渲染面自己 spread 两份：上一轮 mermaid 就是因为
// 「各渲染面各自装配」漏了三处，这一轮链接又漏了消息页。渲染面之间该有的差异
// 只剩「用不用这张表」，不再是「装配得全不全」。
import { createElement, Fragment } from "react";
import type { Components } from "react-markdown";
import { mermaidMarkdownComponents } from "./mermaidComponents";
import { mdLinkComponents } from "./linkComponents";

export const mdComponents: Components = {
  ...mermaidMarkdownComponents,
  ...mdLinkComponents,
};

// 单行面（P 任务行、band 标题这类）要的变体：`p` 展平成 fragment，正文的
// 加粗/`code` 照样渲染，但不产生块级段落把行的压行/省略号打断。桌面那边
// 的对应物是 markdown/safeLinks 的 inlineMarkdownComponents。
export const mdInlineComponents: Components = {
  ...mdComponents,
  p: ({ children }) => createElement(Fragment, null, children),
};
