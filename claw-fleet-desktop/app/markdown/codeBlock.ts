/**
 * Whether a markdown `<code>` node is a fenced/indented code block (as opposed
 * to an inline `` `code` `` span).
 *
 * A block either carries a language info-string (```` ```ts ````) or spans
 * multiple lines. react-markdown v10 dropped the `inline` prop that used to be
 * the discriminator, so a fence with no language tag is otherwise
 * indistinguishable from an inline span — and would fall through to inline-code
 * styling, which word-wraps and collapses the fixed-width formatting of ASCII
 * diagrams. The multi-line signal recovers those: an inline span never carries
 * a literal newline (a soft break renders as a space), whereas a fenced block's
 * content keeps its internal newlines.
 */
export function isFencedBlock(className: string | undefined, codeText: string): boolean {
  if (/language-(\w+)/.test(className || "")) return true;
  return codeText.includes("\n");
}
