/**
 * Claude Code wraps an invoked slash command in an XML-ish envelope before it
 * lands in the transcript as a user turn:
 *
 *   <command-name>/model</command-name>
 *   <command-message>model</command-message>
 *   <command-args>opus</command-args>
 *
 * The markdown renderer used to swallow those tags (rehype-sanitize unwraps
 * unknown elements), so the envelope was invisible by accident. Now that the
 * user bubble renders as plain text — matching the desktop — the tags would
 * show up raw, so collapse the envelope back into the command the person
 * actually typed. Mirrors `parseSlashCommand` in the desktop's
 * components/blocks/UserContent.tsx; the two apps are separate vite packages.
 */
export function parseSlashCommand(text: string): { name: string; args: string } | null {
  const name = /<command-name>([\s\S]*?)<\/command-name>/.exec(text);
  if (!name) return null;
  const args = /<command-args>([\s\S]*?)<\/command-args>/.exec(text);
  return { name: name[1].trim(), args: (args?.[1] ?? "").trim() };
}

/**
 * What the user bubble shows for one user turn: the raw text, except for a
 * slash-command envelope, which collapses to `/name args`. Everything else is
 * left byte-for-byte — the bubble preserves whitespace, so single newlines the
 * markdown renderer used to collapse into one paragraph now survive.
 */
export function userDisplayText(text: string): string {
  const cmd = parseSlashCommand(text);
  if (!cmd) return text;
  return cmd.args ? `${cmd.name} ${cmd.args}` : cmd.name;
}
