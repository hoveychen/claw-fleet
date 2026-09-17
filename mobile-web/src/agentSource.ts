// Agent source branching for mobile, centralized in one place.
//
// These checks lived scattered as `agentSource === "codex"` ternaries in components.
// When written, Fleet had only claude/codex; after dsh joined, every place silently
// treated dsh as Claude—listed Claude models for dsh sessions, read `dsh://` URIs as
// jsonl. Extracted to pure functions so tests can nail each one down; adding a fourth
// source only touches this file.
//
// Desktop equivalent: claw-fleet-desktop/app/modelChoices.ts (toolForAgentSource /
// tokenPanelForAgentSource).
import type { SourceInfo } from "./useSourcesConfig";

/** Agent tools Fleet can launch new sessions with (mirrors desktop AGENT_TOOL_CHOICES).
 *  Values are launcher tool values: Claude source registered as "claude-code" but
 *  tool value is bare "claude"; codex / dsh registration names and tool values match. */
export const AGENT_TOOL_CHOICES: Array<[string, string]> = [
  ["claude", "Claude"],
  ["codex", "Codex"],
  ["dsh", "DeepSeek Harness"],
];

/** Source registration name → launcher tool value. */
function sourceNameToTool(name: string): string {
  return name === "claude-code" ? "claude" : name;
}

/** Constrain tool picker to truly monitored sources (source on **and** available on
 *  host). `null` (config not arrived) or no matches fall back to Claude-only so picker
 *  is never empty and never flashes a monitored tool then hides it. */
export function toolChoicesForSources(
  sources: SourceInfo[] | null,
): Array<[string, string]> {
  const active = new Set(
    (sources ?? []).filter((s) => s.enabled && s.available).map((s) => sourceNameToTool(s.name)),
  );
  const filtered = AGENT_TOOL_CHOICES.filter(([v]) => active.has(v));
  return filtered.length ? filtered : [AGENT_TOOL_CHOICES[0]];
}

/** Which model/effort roster a session's `agentSource` uses. Fleet doesn't guess:
 *  unrecognized sources treat as Claude, the registry's own fallback. */
export function toolForAgentSource(agentSource: string | undefined | null): string {
  const tool = sourceNameToTool((agentSource ?? "").trim());
  return AGENT_TOOL_CHOICES.some(([v]) => v === tool) ? tool : "claude";
}

/** Can a tool row expand details?—expand reads Claude's jsonl by tool_use_id.
 *
 *  Codex rollout has no toolUseResult, folded format breaks scanning; dsh has no
 *  transcript file at all, its `jsonlPath` is a `dsh://<id>` URI, reading it as a
 *  path always fails. Both return undefined = tool row not expandable. */
export function detailPathForSession(
  agentSource: string | undefined | null,
  jsonlPath: string | undefined,
): string | undefined {
  return toolForAgentSource(agentSource) === "claude" ? jsonlPath : undefined;
}

/** Which relay method the token tab should call.
 *
 *  Each source tracks tokens in its own vocabulary, uses its own channel; panel isn't
 *  universal: Claude parses from session JSONL (`token_breakdown`), dsh has no
 *  transcript file at all—its usage needs RPC call via `dsh://<id>` URI
 *  (`dsh_token_breakdown`)—feeding a URI as a path to the file-read method only ever
 *  shows "parse failed". */
export function tokenRequestFor(session: {
  agentSource?: string;
  jsonlPath?: string;
  workspacePath?: string;
}): { method: string; params: Record<string, unknown> } {
  if (toolForAgentSource(session.agentSource) === "dsh") {
    return { method: "dsh_token_breakdown", params: { uri: session.jsonlPath } };
  }
  return {
    method: "token_breakdown",
    params: { path: session.jsonlPath, projectRoot: session.workspacePath },
  };
}
