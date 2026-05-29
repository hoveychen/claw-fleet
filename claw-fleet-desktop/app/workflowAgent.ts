import type { SessionInfo } from "./types";

/**
 * A Claude Code Workflow fan-out agent — its transcript lives under
 * `<session>/subagents/workflows/<run>/agent-*.jsonl`. The session scan surfaces
 * these as subagent sessions so a DAG node can open one, but they must be hidden
 * from the normal session lists/tabs: a single run can spawn 100+ of them and
 * would otherwise flood the UI.
 */
export function isWorkflowAgent(s: SessionInfo): boolean {
  return s.jsonlPath.includes("/subagents/workflows/");
}
