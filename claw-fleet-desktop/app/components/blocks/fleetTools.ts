/**
 * Shared parsing layer for Fleet's MCP *control* tools — `fleet__plan`,
 * `fleet__handoff`, `fleet__watch`, `fleet__loop`, `fleet__schedule`,
 * `fleet__wiki`. These render through the generic tool card by default, which
 * dumps `{"action":"check","plan_id":…}` as a key/value blob. `FleetToolCard`
 * replaces that with a structured, human-readable card.
 *
 * Ground truth from `claw-fleet-core/src/mcp_control.rs`: a control tool's
 * *return text* is NOT uniformly JSON. It comes in four shapes:
 *   - line text : `plan list`/`plan get`, `wiki list`/`wiki search`
 *   - pretty JSON: `handoff`/`watch`/`loop`/`schedule` list/get
 *   - file body : `wiki cat` (markdown / html)
 *   - confirm   : every mutate action (`ok: …`)
 *
 * The only always-structured source is the *input* (`action` + params), so the
 * card leans on that; the return text is classified into `FleetResult` below.
 */

/** The six MCP control tools, keyed by the tail segment of their tool name. */
export const FLEET_CONTROL_TOOLS = [
  "plan",
  "handoff",
  "watch",
  "loop",
  "schedule",
  "wiki",
] as const;

export type FleetTool = (typeof FLEET_CONTROL_TOOLS)[number];

/**
 * Identify a Fleet control tool by name. MCP namespaces the tool, so the wire
 * name is `mcp__fleet__fleet__plan`; a bare `fleet__plan` is also possible.
 * Match on the tail — the same `endsWith` convention `isDecisionTool` uses for
 * `fleet__ask` (see `toolResults.ts`).
 */
export function isFleetTool(name: string): FleetTool | null {
  for (const tool of FLEET_CONTROL_TOOLS) {
    if (name.endsWith(`fleet__${tool}`)) return tool;
  }
  return null;
}

/**
 * i18n key for a Fleet MCP tool's human-readable label, keyed by the tail
 * segment of its wire name (`mcp__fleet__fleet__<tail>`). Covers the six
 * control tools plus `ask` / `render_a2ui` / `set_session_title`. Used to
 * relabel the raw `mcp__fleet__fleet__…` id wherever it would otherwise leak
 * verbatim — e.g. the ToolSearch "loading tools" summary, where a tool is just
 * a string in the `select:` query and never reaches its dedicated card.
 */
export const FLEET_TOOL_LABEL_KEYS: Record<string, string> = {
  ask: "detail.fleet_tool.ask",
  render_a2ui: "detail.fleet_tool.render_a2ui",
  plan: "detail.fleet_tool.plan",
  handoff: "detail.fleet_tool.handoff",
  watch: "detail.fleet_tool.watch",
  loop: "detail.fleet_tool.loop",
  schedule: "detail.fleet_tool.schedule",
  wiki: "detail.fleet_tool.wiki",
  set_session_title: "detail.fleet_tool.set_session_title",
};

/**
 * Human-friendly label for a raw tool id (as it appears in a ToolSearch
 * `select:` list). Fleet's MCP tools (`mcp__fleet__fleet__ask`, …) map to a
 * translated label; any other MCP tool (`mcp__<server>__<tool>`) drops the
 * `mcp__server__` prefix and renders `server·tool`; a plain non-MCP tool name
 * passes through unchanged.
 */
export function friendlyToolName(rawId: string, t: (key: string) => string): string {
  const id = rawId.trim();
  for (const [tail, key] of Object.entries(FLEET_TOOL_LABEL_KEYS)) {
    if (id === `fleet__${tail}` || id.endsWith(`fleet__fleet__${tail}`)) return t(key);
  }
  if (id.startsWith("mcp__")) {
    const parts = id.split("__");
    if (parts.length >= 3) return `${parts[1]}·${parts.slice(2).join("__")}`;
  }
  return id;
}

// ── Result shapes ────────────────────────────────────────────────────────────

export interface PlanListItem {
  id: string;
  done: number;
  total: number;
  source?: string;
}

export interface PlanGetItem {
  done: boolean;
  text: string;
}

export interface WikiListItem {
  slug: string;
  kind: string;
  versions: string;
  title: string;
}

export interface WikiSearchItem {
  slug: string;
  field: string;
  matched: string;
}

/**
 * Classified return text. `confirm` is the `ok: …` line of a mutate; `records`
 * holds the parsed JSON array for handoff/watch/loop/schedule list/get; `raw`
 * is the untouched text when parsing didn't apply or failed (never lose data).
 */
export type FleetResult =
  | { kind: "confirm"; text: string }
  | { kind: "plan-list"; plans: PlanListItem[] }
  | { kind: "plan-get"; items: PlanGetItem[] }
  | { kind: "wiki-list"; docs: WikiListItem[] }
  | { kind: "wiki-search"; hits: WikiSearchItem[] }
  | { kind: "wiki-cat"; body: string }
  | { kind: "records"; records: Record<string, unknown>[] }
  | { kind: "error"; text: string }
  | { kind: "raw"; text: string }
  | { kind: "none" };

export interface FleetView {
  tool: FleetTool;
  /** The `action` from input, e.g. "check". Empty when input carried none. */
  action: string;
  /** The raw input — the card pulls per-tool params off this. */
  input: Record<string, unknown>;
  result: FleetResult;
}

// ── Line parsers (text-shaped returns) ───────────────────────────────────────

/** `plan list`: `id [done/total]` optionally followed by ` — source`. */
export function parsePlanList(text: string): PlanListItem[] {
  const out: PlanListItem[] = [];
  for (const line of text.split("\n")) {
    const m = /^(.+?) \[(\d+)\/(\d+)\](?: — (.+))?$/.exec(line.trim());
    if (!m) continue;
    out.push({
      id: m[1],
      done: Number(m[2]),
      total: Number(m[3]),
      source: m[4],
    });
  }
  return out;
}

/** `plan get`: `[x] text` / `[ ] text` per line. */
export function parsePlanGet(text: string): PlanGetItem[] {
  const out: PlanGetItem[] = [];
  for (const line of text.split("\n")) {
    const m = /^\[([ x])\] (.*)$/.exec(line.trim());
    if (!m) continue;
    out.push({ done: m[1] === "x", text: m[2] });
  }
  return out;
}

/** `wiki list`: `slug  [kind]  vN  title`. */
export function parseWikiList(text: string): WikiListItem[] {
  const out: WikiListItem[] = [];
  for (const line of text.split("\n")) {
    const m = /^(\S+)\s+\[([^\]]*)\]\s+(v\S+)\s+(.*)$/.exec(line.trim());
    if (!m) continue;
    out.push({ slug: m[1], kind: m[2], versions: m[3], title: m[4] });
  }
  return out;
}

/** `wiki search`: `slug  [field]  matched`. */
export function parseWikiSearch(text: string): WikiSearchItem[] {
  const out: WikiSearchItem[] = [];
  for (const line of text.split("\n")) {
    const m = /^(\S+)\s+\[([^\]]*)\]\s+(.*)$/.exec(line.trim());
    if (!m) continue;
    out.push({ slug: m[1], field: m[2], matched: m[3] });
  }
  return out;
}

/** Parse a pretty-JSON return into an array of records (single object → [obj]). */
function tryParseRecords(text: string): Record<string, unknown>[] | null {
  const trimmed = text.trim();
  if (!trimmed.startsWith("[") && !trimmed.startsWith("{")) return null;
  try {
    const parsed: unknown = JSON.parse(trimmed);
    if (Array.isArray(parsed)) {
      return parsed.filter(
        (r): r is Record<string, unknown> =>
          typeof r === "object" && r !== null && !Array.isArray(r),
      );
    }
    if (typeof parsed === "object" && parsed !== null) {
      return [parsed as Record<string, unknown>];
    }
    return null;
  } catch {
    return null;
  }
}

/** Actions whose return is a serialized JSON record list, per tool. */
function isJsonRecordAction(tool: FleetTool, action: string): boolean {
  if (action !== "list" && action !== "get") return false;
  return (
    tool === "handoff" || tool === "watch" || tool === "loop" || tool === "schedule"
  );
}

/**
 * Classify a control tool's return text into a structured `FleetResult`. Falls
 * back to `raw` (never drops the text) when a parse doesn't apply or fails.
 */
export function classifyResult(
  tool: FleetTool,
  action: string,
  content: string,
  isError: boolean,
): FleetResult {
  if (isError) return { kind: "error", text: content };
  const text = content.trim();
  if (!text) return { kind: "none" };

  if (tool === "plan" && action === "list") {
    const plans = parsePlanList(text);
    return plans.length ? { kind: "plan-list", plans } : { kind: "raw", text };
  }
  if (tool === "plan" && action === "get") {
    const items = parsePlanGet(text);
    return items.length ? { kind: "plan-get", items } : { kind: "raw", text };
  }
  if (tool === "wiki" && action === "list") {
    const docs = parseWikiList(text);
    return docs.length ? { kind: "wiki-list", docs } : { kind: "raw", text };
  }
  if (tool === "wiki" && action === "search") {
    const hits = parseWikiSearch(text);
    return hits.length ? { kind: "wiki-search", hits } : { kind: "raw", text };
  }
  if (tool === "wiki" && action === "cat") {
    return { kind: "wiki-cat", body: content };
  }
  if (isJsonRecordAction(tool, action)) {
    const records = tryParseRecords(text);
    return records ? { kind: "records", records } : { kind: "raw", text };
  }
  // Mutate actions (create/check/uncheck/add/resume/stop/cancel/update/run) and
  // anything else: the `ok: …` confirmation line.
  return { kind: "confirm", text };
}

/** Pull the plain string out of a `tool_result.content` (string | blocks). */
export function resultText(content: string | unknown[]): string {
  return typeof content === "string" ? content : "";
}

/**
 * Build the view model from a tool_use block's input and its result text.
 * `tool` is the identified control tool; `input` is the raw tool_use input;
 * `content`/`isError` come off the matching tool_result.
 */
export function parseFleetCall(
  tool: FleetTool,
  input: Record<string, unknown>,
  content: string,
  isError: boolean,
): FleetView {
  const action = typeof input.action === "string" ? input.action : "";
  return {
    tool,
    action,
    input,
    result: classifyResult(tool, action, content, isError),
  };
}
