import type { RawMessage } from "../types";

/** A subagent's own transcript lives in a separate file next to its parent's
 *  (`<project>/<parent-session>/subagents/agent-*.jsonl`) and **every** line in
 *  it carries `isSidechain: true` — the flag marks "this row belongs to a
 *  sidechain", not "this row is foreign to the file it sits in". */
export function isSubagentTranscript(jsonlPath: string): boolean {
  return jsonlPath.includes("/subagents/");
}

/** Rows to render in the 消息 list.
 *
 *  Sidechain rows are dropped from a **main** session's list: older Claude Code
 *  wrote a subagent's turns inline into the parent transcript, and replaying
 *  them there interleaves two conversations. But when the transcript being
 *  viewed *is* the subagent's own file, that same filter empties the whole view
 *  — which is exactly what the mobile detail page did until 2026-09-10
 *  ("暂无可显示的消息" on every subagent drill-down). So the filter is scoped to
 *  the transcript it was written for. */
export function filterMainRows(rows: RawMessage[], jsonlPath: string): RawMessage[] {
  if (isSubagentTranscript(jsonlPath)) return rows;
  return rows.filter((m) => !m.isSidechain);
}
