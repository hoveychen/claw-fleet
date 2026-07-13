// Parsing for the background-agent completion notice Claude Code injects as a
// user turn. Kept in its own module (no React/CSS imports) so it stays cheap to
// unit-test and mirrors the desktop's parseTaskNotification.
//
//   <task-notification><status>completed</status>
//   <summary>Agent "…" finished</summary><result>…markdown…</result>
//   </task-notification>

export interface ParsedTaskNotification {
  taskId?: string;
  outputFile?: string;
  status?: string;
  summary?: string;
  result?: string;
}

export function parseTaskNotification(text: string): ParsedTaskNotification | null {
  if (!text.includes("<task-notification>")) return null;
  const pick = (tag: string): string | undefined => {
    const m = new RegExp(`<${tag}>([\\s\\S]*?)</${tag}>`).exec(text);
    return m ? m[1].trim() : undefined;
  };
  const parsed: ParsedTaskNotification = {
    taskId: pick("task-id"),
    outputFile: pick("output-file"),
    status: pick("status"),
    summary: pick("summary"),
    result: pick("result"),
  };
  if (!parsed.summary && !parsed.result && !parsed.status) return null;
  return parsed;
}

/** `summary` reads like `Agent "…" finished`; the quoted run is the agent's own
 *  label and makes a tighter title than the full sentence. */
export function taskTitle(summary?: string): string {
  if (!summary) return "Agent";
  const q = /["“]([^"”]+)["”]/.exec(summary);
  return q ? q[1] : summary;
}

export function basename(path: string): string {
  const i = path.lastIndexOf("/");
  return i >= 0 ? path.slice(i + 1) : path;
}
