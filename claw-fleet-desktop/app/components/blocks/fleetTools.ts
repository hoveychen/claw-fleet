/**
 * Shared parsing layer for Fleet's MCP *control* tools — `fleet__plan`,
 * `fleet__handoff`, `fleet__watch`, `fleet__loop`, `fleet__schedule`,
 * `fleet__wiki`, `fleet__artifact`, `fleet__inspect`, `fleet__control`,
 * `fleet__notes`, `fleet__history`. These render through the generic tool card
 * by default, which dumps `{"action":"check","plan_id":…}` as a key/value blob
 * — and in the work-run rail, where the tool name is hidden, that blob is the
 * *entire* row (`{"action":"list","all":true}` and nothing else).
 * `FleetToolCard` replaces it with a structured, human-readable card.
 *
 * Ground truth from `claw-fleet-core/src/mcp_control.rs` +
 * `mcp_inspect.rs`: a control tool's *return text* is NOT uniformly JSON. It
 * comes in five shapes:
 *   - line text : `plan list`/`plan get`, `wiki list`/`wiki search`
 *   - pretty JSON: `handoff`/`watch`/`loop`/`schedule` list/get
 *   - file body : `wiki cat` (markdown / html), `notes read`
 *   - prose text: every `inspect`/`history` action, `notes list`/`search`,
 *                 `artifact list`/`get` — already formatted for the eye by the
 *                 Rust side, so it is passed through verbatim
 *   - confirm   : every mutate action (`ok: …` / `Stored artifact …`)
 *
 * The only always-structured source is the *input* (`action` + params), so the
 * card leans on that; the return text is classified into `FleetResult` below.
 */

/**
 * The MCP control tools, keyed by the tail segment of their tool name. Mirrors
 * `CONTROL_TOOL_NAMES` in `claw-fleet-core/src/mcp_control.rs` — a tool missing
 * from this list falls back to the generic card and leaks its raw args JSON, so
 * the two lists must be kept in step.
 */
export const FLEET_CONTROL_TOOLS = [
  "plan",
  "handoff",
  "watch",
  "loop",
  "schedule",
  "wiki",
  "artifact",
  "inspect",
  "control",
  "notes",
  "history",
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
 * segment of its wire name (`mcp__fleet__fleet__<tail>`). Covers every one of
 * the eleven control tools plus the four non-control ones (`ask`,
 * `render_a2ui`, `set_session_title`, `image`, `image_edit`). Used to relabel
 * the raw `mcp__fleet__fleet__…` id wherever it would otherwise leak verbatim —
 * e.g. the ToolSearch "loading tools" summary, where a tool is just a string in
 * the `select:` query and never reaches its dedicated card, and which is where
 * the seven previously-missing entries surfaced as `fleet·fleet__inspect`.
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
  artifact: "detail.fleet_tool.artifact",
  inspect: "detail.fleet_tool.inspect",
  control: "detail.fleet_tool.control",
  notes: "detail.fleet_tool.notes",
  history: "detail.fleet_tool.history",
  set_session_title: "detail.fleet_tool.set_session_title",
  image: "detail.fleet_tool.image",
  image_edit: "detail.fleet_tool.image_edit",
  permission_prompt: "detail.fleet_tool.permission_prompt",
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

/**
 * Is this call the moment a deliverable entered the 产出 store or a doc entered
 * the 知识库?
 *
 * Both are "the run produced a thing you can hold", which is a different kind
 * of event from the reads and edits around them — so, like a decision card,
 * such a record is never swept into a collapsed work band (`isWorkRow`). One
 * predicate rather than two `endsWith` checks at each site, because the fold
 * rule and the card renderer must agree on exactly which calls are ingests.
 */
export function isIngestCall(name: string, input: unknown): boolean {
  const tool = isFleetTool(name);
  if (tool !== "artifact" && tool !== "wiki") return false;
  const action =
    typeof input === "object" && input !== null
      ? (input as Record<string, unknown>).action
      : undefined;
  return tool === "artifact" ? action === "add" : action === "publish";
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
 * A deliverable that just landed in the 产出 store, parsed out of `artifact
 * add`'s confirmation line. The id is the load-bearing field: it is what lets
 * the card fetch the artifact's metadata and render its actual content, rather
 * than restating the sentence the tool already returned.
 */
export interface ArtifactAdded {
  id: string;
  title: string;
  /** The store's coarse bucket (`pdf`, `image`, `video`, `markdown`, …). */
  artifactKind: string;
  bytes: number;
}

/** A doc that just landed in the 知识库, parsed out of `wiki publish`'s line. */
export interface WikiPublished {
  slug: string;
  version: string;
  title: string;
}

/**
 * Classified return text. `confirm` is the `ok: …` line of a mutate; `records`
 * holds the parsed JSON array for handoff/watch/loop/schedule list/get; `raw`
 * is the untouched text when parsing didn't apply or failed (never lose data).
 */
export type FleetResult =
  | { kind: "confirm"; text: string }
  | { kind: "artifact-add"; artifact: ArtifactAdded }
  | { kind: "wiki-publish"; doc: WikiPublished }
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

/**
 * `artifact add` → `Stored artifact <id> — <title> (<kind>, <n> bytes), copied.
 * It is now on the 产出 page.` (`mcp_control.rs::handle_artifact`).
 *
 * A title is free text and may itself contain " (" or " — ", so the shape is
 * matched from the tail: the `(<kind>, <n> bytes)` group is the anchor and the
 * title is whatever lies between the em dash and it.
 */
export function parseArtifactAdd(text: string): ArtifactAdded | null {
  const m = /^Stored artifact (\S+) — (.*) \(([^(),]+), (\d+) bytes\)/.exec(text.trim());
  if (!m) return null;
  return { id: m[1], title: m[2], artifactKind: m[3], bytes: Number(m[4]) };
}

/** `wiki publish` → `Published <slug> (version <v>, <n> total). title: <title>`. */
export function parseWikiPublish(text: string): WikiPublished | null {
  const m = /^Published (\S+) \(version (\S+), \d+ total\)\. title: (.*)$/.exec(text.trim());
  if (!m) return null;
  return { slug: m[1], version: m[2], title: m[3] };
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
 * The five tools whose returns the Rust side already formats for the eye
 * (`mcp_inspect.rs` agent tables, `render_note_files`, `render_history_hits`,
 * the artifact listing) rather than as parseable records. Their read actions go
 * straight to `raw`, which renders in a `<pre>` and so keeps the alignment the
 * Rust formatter built. The remaining actions — listed per tool below — are
 * mutates that return one `ok: …` / `Stored artifact …` line and read better as
 * a `confirm`. Splitting them is the point: the pre-existing catch-all sent
 * *every* unrecognised action to `confirm`, whose single-line div would have
 * collapsed a multi-line agent table into an unreadable run of text.
 */
const PROSE_TOOL_MUTATES: Partial<Record<FleetTool, readonly string[]>> = {
  inspect: [],
  history: [],
  notes: ["write", "append"],
  artifact: ["add", "delete"],
  control: ["stop", "interrupt"],
};

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
  // The two ingest confirmations are the only mutates whose *subject* the
  // reader wants to see rather than be told about — a deliverable landing in
  // the 产出 store, a doc landing in the 知识库. Parsed into an identifier the
  // card can resolve into the real thing; an unrecognised sentence (an older
  // core, a future wording) falls back to the plain confirm line.
  if (tool === "artifact" && action === "add") {
    const artifact = parseArtifactAdd(text);
    return artifact ? { kind: "artifact-add", artifact } : { kind: "confirm", text };
  }
  if (tool === "wiki" && action === "publish") {
    const doc = parseWikiPublish(text);
    return doc ? { kind: "wiki-publish", doc } : { kind: "confirm", text };
  }
  if (tool === "wiki" && action === "cat") {
    return { kind: "wiki-cat", body: content };
  }
  if (isJsonRecordAction(tool, action)) {
    const records = tryParseRecords(text);
    return records ? { kind: "records", records } : { kind: "raw", text };
  }
  // `notes read` returns the note file's body — the same "a file, verbatim"
  // shape as `wiki cat`, so it gets the same markdown rendering. Notes are
  // markdown by convention (`checkpoint.md`).
  if (tool === "notes" && action === "read") {
    return { kind: "wiki-cat", body: content };
  }
  const mutates = PROSE_TOOL_MUTATES[tool];
  if (mutates) {
    return mutates.includes(action) ? { kind: "confirm", text } : { kind: "raw", text };
  }
  // Mutate actions (create/check/uncheck/add/resume/stop/cancel/update/run) and
  // anything else: the `ok: …` confirmation line.
  return { kind: "confirm", text };
}

/**
 * Pull the plain string out of a `tool_result.content` (string | blocks).
 *
 * The block-array form is not an edge case — it is the *only* shape an MCP
 * tool returns (`[{"type":"text","text":"Stored artifact …"}]`), so dropping it
 * meant every Fleet control tool's result was silently empty: no preview card
 * for an ingest, no progress bars for `plan list`, no rows for `wiki list`.
 * The card still rendered its header summary off the *input*, which is why it
 * looked fine for months. Tests fed strings, so they stayed green.
 */
export function resultText(content: string | unknown[]): string {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content
    .map((b) =>
      typeof b === "object" && b !== null && typeof (b as { text?: unknown }).text === "string"
        ? (b as { text: string }).text
        : "",
    )
    .join("");
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
