// Component map for markdown bodies, shared by all render surfaces.
//
// Two sources: Mermaid fences rendering diagrams (mermaidComponents) and
// clickable links (linkComponents). We combine them into one table rather than
// have each surface assemble its own: the last round of Mermaid was missed in
// three places due to "each surface assembles its own", and this round links
// were missed on the message page. The only variance between surfaces should
// now be "do you use this map", not "how complete is your assembly".
import { createElement, Fragment } from "react";
import type { Components } from "react-markdown";
import { mermaidMarkdownComponents } from "./mermaidComponents";
import { mdLinkComponents } from "./linkComponents";
import { ExplainMarkSpan } from "./explainMarks";

export const mdComponents: Components = {
  ...mermaidMarkdownComponents,
  ...mdLinkComponents,
  // The agent's `[?text]` marks: tappable inside an ExplainMarksProvider,
  // plain text elsewhere. See ./explainMarks.
  span: ExplainMarkSpan,
};

// Variant for single-line surfaces (task rows, band titles, etc.): `p` flattens
// to a fragment, so bold/**code** still render but don't break lines with a
// block paragraph that collides with line clamping or ellipsis. The desktop
// equivalent is inlineMarkdownComponents in markdown/safeLinks.
export const mdInlineComponents: Components = {
  ...mdComponents,
  p: ({ children }) => createElement(Fragment, null, children),
};
