/**
 * Messages one Fleet session sends another (`fleet__control action=send`,
 * `fleet send`) arrive as an ordinary user turn with a return address appended
 * by `with_reply_address` in `claw-fleet-core/src/mcp_inspect.rs`:
 *
 *   <body>
 *
 *   ---
 *   （这条消息来自 Fleet 会话 <id>（<workspace>）。要回复它，用 `fleet__control …`（CLI：`fleet send <id> <回复>`）。）
 *
 * The footer is for the receiving model — it is how it knows whom to answer —
 * so it stays in the transcript. The transcript views lift it out instead and
 * render the turn as incoming mail from that session rather than as something
 * the user typed. Several queued messages can be joined into one resume prompt
 * (`pending_message.rs`, "\n\n"), so one turn may hold more than one footer.
 */

export interface AgentMailSender {
  id: string;
  /** Workspace name the sender was scanned under; absent when it wasn't. */
  workspace?: string;
}

export interface AgentMailPart {
  body: string;
  /** Who sent this part; absent for trailing text that carried no footer. */
  from?: AgentMailSender;
}

const FOOTER_RE =
  /\n*---\n（这条消息来自 Fleet 会话 ([A-Za-z0-9_-]+)(?:（([^）\n]*)）)?。要回复它，[^\n]*?<回复>`）。）/g;

/** Split a user turn into its mail parts, or `null` when it carries no
 *  reply-address footer at all (the overwhelmingly common case). */
export function splitAgentMail(text: string): AgentMailPart[] | null {
  if (!text.includes("这条消息来自 Fleet 会话")) return null;
  const parts: AgentMailPart[] = [];
  let last = 0;
  for (const m of text.matchAll(FOOTER_RE)) {
    const at = m.index ?? 0;
    const body = text.slice(last, at).trim();
    const from: AgentMailSender = { id: m[1] };
    if (m[2]) from.workspace = m[2];
    parts.push({ body, from });
    last = at + m[0].length;
  }
  if (parts.length === 0) return null;
  const rest = text.slice(last).trim();
  if (rest) parts.push({ body: rest });
  return parts;
}

/** The text worth copying out of a mail turn: the bodies, footers dropped. */
export function agentMailPlainText(parts: AgentMailPart[]): string {
  return parts
    .map((p) => p.body)
    .filter(Boolean)
    .join("\n\n");
}
