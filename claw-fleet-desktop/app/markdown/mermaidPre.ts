/**
 * Decision logic for overriding `<pre>`: does this `<pre>` wrap a ```mermaid fence?
 *
 * Every markdown render point replaces mermaid fences with `<MermaidBlock>` at the
 * `code` component level, but react-markdown still generates the outer `<pre>`,
 * trapping the diagram in a monospace, background-filled block. Font inheritance
 * makes mermaid's measured label width wrong (see fontFamily comment in
 * MermaidBlock), and the background stacks with the diagram's own card border,
 * creating a double frame. So the mount point adds a `pre` override that only
 * strips the outer wrapper for mermaid fences, leaving other fences alone.
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
