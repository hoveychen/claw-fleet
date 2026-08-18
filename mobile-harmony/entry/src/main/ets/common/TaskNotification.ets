/// Parsing for the background-agent completion notice Claude Code injects as a
/// user turn — the native port of mobile-web/src/views/taskNotification.ts.
///
///   <task-notification><status>completed</status>
///   <summary>Agent "…" finished</summary><result>…markdown…</result>
///   </task-notification>

export interface ParsedTaskNotification {
  taskId?: string;
  outputFile?: string;
  status?: string;
  summary?: string;
  result?: string;
}

/// Pull the trimmed inner text of `<tag>…</tag>`, or undefined when absent.
function pickTag(text: string, tag: string): string | undefined {
  const re = new RegExp(`<${tag}>([\\s\\S]*?)</${tag}>`);
  const m = re.exec(text);
  return m !== null ? m[1].trim() : undefined;
}

export function parseTaskNotification(text: string): ParsedTaskNotification | null {
  if (!text.includes('<task-notification>')) {
    return null;
  }
  const parsed: ParsedTaskNotification = {
    taskId: pickTag(text, 'task-id'),
    outputFile: pickTag(text, 'output-file'),
    status: pickTag(text, 'status'),
    summary: pickTag(text, 'summary'),
    result: pickTag(text, 'result'),
  };
  if (parsed.summary === undefined && parsed.result === undefined && parsed.status === undefined) {
    return null;
  }
  return parsed;
}

/// `summary` reads like `Agent "…" finished`; the quoted run is the agent's own
/// label and makes a tighter title than the full sentence.
export function taskTitle(summary?: string): string {
  if (summary === undefined || summary.length === 0) {
    return 'Agent';
  }
  const m = /["“]([^"”]+)["”]/.exec(summary);
  return m !== null ? m[1] : summary;
}

export function taskBasename(path: string): string {
  const i = path.lastIndexOf('/');
  return i >= 0 ? path.substring(i + 1) : path;
}
