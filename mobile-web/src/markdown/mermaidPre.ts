/**
 * Predicate for `pre` override: does this `<pre>` wrap a ```mermaid fence.
 *
 * Kept in sync with desktop claw-fleet-desktop/app/markdown/mermaidPre.ts (both apps are
 * independent vite packages, logic is duplicated not shared; unit tests are on desktop side).
 *
 * Every markdown render point swaps mermaid fence for <MermaidBlock> on the `code` component,
 * but react-markdown's generated outer `<pre>` is still there, so the diagram gets stuffed
 * into a monospace-font block with its own background: font inheritance makes mermaid's
 * measured label width mismatch (see fontFamily comment in MermaidBlock), and the background
 * doubles into two nested frames with the diagram's own card. So we add one more `pre` override
 * at the mount point: strip the outer layer only for mermaid fence, leave other fences as-is.
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
