/**
 * Typed access to Claude Code's `toolUseResult` — the structured payload it
 * writes on the transcript record that carries a tool's `tool_result` block.
 *
 * Why this module exists: `tool_result.content` is a stringified blob, while
 * `toolUseResult` carries the real thing (unified-diff hunks, separated
 * stdout/stderr, the answered questions of an AskUserQuestion card). Rendering
 * a tool card from the blob throws away everything Claude Code already knows.
 *
 * Two hard constraints shape every function below:
 *
 * 1. `toolUseResult` is an *internal* Claude Code field, not a public contract.
 *    Key sets differ per tool and may change across CLI versions.
 * 2. Only the Claude agent source emits it. Codex, OpenClaw and the generic
 *    agent source never do.
 *
 * So each guard validates the shape it needs and returns `null` otherwise. A
 * `null` must always mean "fall back to the generic tool rendering", never
 * "render nothing".
 *
 * One `null` is expected rather than exceptional: **when a tool call fails
 * (`tool_result.is_error`), `toolUseResult` degrades from the structured object
 * to a bare error string.** Measured across 120 transcripts, every one of the
 * 474 payloads that failed a guard was an errored call. Read those with
 * `asErrorMessage` instead.
 *
 * Measured shapes (120 transcripts, 21 tools, 11k+ calls) are catalogued in the
 * wiki doc `claude-transcript-tooluseresult`.
 */

import type { RawMessage, ToolResultBlock } from "./types";

// ── Per-tool payload shapes ──────────────────────────────────────────────────

/** `Bash`. Note there is no exit code and no duration — the transcript simply
 *  doesn't record them for Bash (Glob / WebFetch / WebSearch / Agent do carry
 *  timings). Error state lives on `tool_result.is_error`, not here. */
export interface BashToolResult {
  stdout: string;
  stderr: string;
  interrupted: boolean;
  isImage?: boolean;
  noOutputExpected?: boolean;
}

/** One unified-diff hunk. `lines` entries are prefixed with " ", "+" or "-". */
export interface PatchHunk {
  oldStart: number;
  oldLines: number;
  newStart: number;
  newLines: number;
  lines: string[];
}

/** `Edit` and `Write` both carry the pre-edit file text plus real diff hunks,
 *  which is why the transcript never needs to be replayed to reconstruct a
 *  baseline. */
export interface FileEditToolResult {
  filePath: string;
  structuredPatch: PatchHunk[];
  originalFile?: string;
  userModified?: boolean;
}

/** `Agent` — a subagent run. Everything a rich subagent card needs. */
export interface AgentToolResult {
  status: string;
  agentId?: string;
  agentType?: string;
  resolvedModel?: string;
  totalDurationMs?: number;
  totalTokens?: number;
  totalToolUseCount?: number;
  prompt?: string;
  content?: string;
  toolStats?: Record<string, number>;
}

export interface AskQuestionOption {
  label: string;
  description?: string;
  preview?: string;
}

export interface AskQuestion {
  question: string;
  header?: string;
  multiSelect?: boolean;
  options?: AskQuestionOption[];
}

/**
 * `AskUserQuestion` and `mcp__fleet__fleet__ask`.
 *
 * `answers` is keyed by the *full question text* (speech divider, markdown and
 * all); values are the chosen option labels. For `fleet__ask` the map also
 * carries form-field answers keyed by field name, flattened into the same
 * object — question keys are prose, field names are identifiers, so they don't
 * collide.
 */
export interface AskUserQuestionToolResult {
  questions: AskQuestion[];
  answers: Record<string, string>;
}

export interface TodoItem {
  content: string;
  status: string;
  activeForm?: string;
}

export interface TodoWriteToolResult {
  oldTodos: TodoItem[];
  newTodos: TodoItem[];
}

/** `Glob` and `Grep` — the two shapes overlap enough to share a reader. */
export interface FileSearchToolResult {
  filenames: string[];
  numFiles: number;
  /** `Grep` only. */
  numMatches?: number;
  /** `Grep` only: decides whether `filenames` or `content` is meaningful. */
  mode?: string;
  /** `Glob` only. */
  totalMatches?: number;
  truncated?: boolean;
  durationMs?: number;
}

// ── Narrowing primitives ─────────────────────────────────────────────────────

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function str(v: unknown): string | undefined {
  return typeof v === "string" ? v : undefined;
}

function num(v: unknown): number | undefined {
  return typeof v === "number" && Number.isFinite(v) ? v : undefined;
}

/**
 * `mcp__fleet__fleet__ask` stores its payload as a JSON *string* rather than an
 * object (measured: 50/50 calls). Unwrap one level so the guards below can
 * treat both decision tools uniformly.
 */
function asObject(v: unknown): Record<string, unknown> | null {
  if (isRecord(v)) return v;
  if (typeof v === "string") {
    const trimmed = v.trim();
    if (!trimmed.startsWith("{")) return null;
    try {
      const parsed: unknown = JSON.parse(trimmed);
      return isRecord(parsed) ? parsed : null;
    } catch {
      return null;
    }
  }
  return null;
}

// ── Guards ───────────────────────────────────────────────────────────────────

export function asBashResult(v: unknown): BashToolResult | null {
  if (!isRecord(v)) return null;
  // stdout/stderr are the load-bearing pair; a Bash payload always has both,
  // and requiring them keeps a same-shaped foreign payload from matching.
  const stdout = str(v.stdout);
  const stderr = str(v.stderr);
  if (stdout === undefined || stderr === undefined) return null;
  return {
    stdout,
    stderr,
    interrupted: v.interrupted === true,
    isImage: v.isImage === true,
    noOutputExpected: v.noOutputExpected === true,
  };
}

function asPatchHunk(v: unknown): PatchHunk | null {
  if (!isRecord(v)) return null;
  const oldStart = num(v.oldStart);
  const oldLines = num(v.oldLines);
  const newStart = num(v.newStart);
  const newLines = num(v.newLines);
  if (
    oldStart === undefined || oldLines === undefined ||
    newStart === undefined || newLines === undefined ||
    !Array.isArray(v.lines) || !v.lines.every((l) => typeof l === "string")
  ) {
    return null;
  }
  return { oldStart, oldLines, newStart, newLines, lines: v.lines as string[] };
}

/** Accepts both `Edit` and `Write` payloads — they share these keys. */
export function asFileEditResult(v: unknown): FileEditToolResult | null {
  if (!isRecord(v)) return null;
  const filePath = str(v.filePath);
  if (filePath === undefined || !Array.isArray(v.structuredPatch)) return null;
  const hunks: PatchHunk[] = [];
  for (const raw of v.structuredPatch) {
    const hunk = asPatchHunk(raw);
    if (!hunk) return null; // a malformed hunk poisons the diff — fall back
    hunks.push(hunk);
  }
  return {
    filePath,
    structuredPatch: hunks,
    originalFile: str(v.originalFile),
    userModified: v.userModified === true,
  };
}

export function asAgentResult(v: unknown): AgentToolResult | null {
  if (!isRecord(v)) return null;
  const status = str(v.status);
  if (status === undefined) return null;
  let toolStats: Record<string, number> | undefined;
  if (isRecord(v.toolStats)) {
    toolStats = {};
    for (const [k, val] of Object.entries(v.toolStats)) {
      const n = num(val);
      if (n !== undefined) toolStats[k] = n;
    }
  }
  return {
    status,
    agentId: str(v.agentId),
    agentType: str(v.agentType),
    resolvedModel: str(v.resolvedModel),
    totalDurationMs: num(v.totalDurationMs),
    totalTokens: num(v.totalTokens),
    totalToolUseCount: num(v.totalToolUseCount),
    prompt: str(v.prompt),
    content: str(v.content),
    toolStats,
  };
}

function asAskQuestion(v: unknown): AskQuestion | null {
  if (!isRecord(v)) return null;
  const question = str(v.question);
  if (question === undefined) return null;
  let options: AskQuestionOption[] | undefined;
  if (Array.isArray(v.options)) {
    options = [];
    for (const raw of v.options) {
      if (!isRecord(raw)) continue;
      const label = str(raw.label);
      if (label === undefined) continue;
      options.push({
        label,
        description: str(raw.description),
        preview: str(raw.preview),
      });
    }
  }
  return {
    question,
    header: str(v.header),
    multiSelect: v.multiSelect === true,
    options,
  };
}

/**
 * `AskUserQuestion` gives a full `{questions, answers}` object. `fleet__ask`
 * gives a JSON string holding only `{answers}` — its questions must come from
 * the `tool_use` input instead, so `questions` is allowed to be absent here and
 * the caller supplies them.
 */
export function asAskUserQuestionResult(
  v: unknown,
): AskUserQuestionToolResult | null {
  const obj = asObject(v);
  if (!obj) return null;
  if (!isRecord(obj.answers)) return null;
  const answers: Record<string, string> = {};
  for (const [k, val] of Object.entries(obj.answers)) {
    if (typeof val === "string") answers[k] = val;
    else if (val != null) answers[k] = String(val);
  }
  const questions: AskQuestion[] = [];
  if (Array.isArray(obj.questions)) {
    for (const raw of obj.questions) {
      const q = asAskQuestion(raw);
      if (q) questions.push(q);
    }
  }
  return { questions, answers };
}

export function asTodoWriteResult(v: unknown): TodoWriteToolResult | null {
  if (!isRecord(v)) return null;
  if (!Array.isArray(v.newTodos)) return null;
  const read = (arr: unknown): TodoItem[] => {
    if (!Array.isArray(arr)) return [];
    const out: TodoItem[] = [];
    for (const raw of arr) {
      if (!isRecord(raw)) continue;
      const content = str(raw.content);
      const status = str(raw.status);
      if (content === undefined || status === undefined) continue;
      out.push({ content, status, activeForm: str(raw.activeForm) });
    }
    return out;
  };
  return { oldTodos: read(v.oldTodos), newTodos: read(v.newTodos) };
}

export function asFileSearchResult(v: unknown): FileSearchToolResult | null {
  if (!isRecord(v)) return null;
  const numFiles = num(v.numFiles);
  if (numFiles === undefined || !Array.isArray(v.filenames)) return null;
  return {
    filenames: v.filenames.filter((f): f is string => typeof f === "string"),
    numFiles,
    numMatches: num(v.numMatches),
    mode: str(v.mode),
    totalMatches: num(v.totalMatches),
    truncated: v.truncated === true,
    durationMs: num(v.durationMs),
  };
}

// ── Failed calls ─────────────────────────────────────────────────────────────

/**
 * A failed tool call replaces the structured payload with a bare error string
 * (`"Error: File has not been read yet…"`, `"InputValidationError: …"`). Every
 * structured guard rejects these, so reach for this one when
 * `tool_result.is_error` is set.
 */
export function asErrorMessage(v: unknown): string | null {
  return typeof v === "string" && v.length > 0 ? v : null;
}

/**
 * Bash records no exit code on success — but a *failed* Bash call opens its
 * error string with `Error: Exit code <n>`. That's the only place the code
 * appears, so a card can show it on failure and nothing on success.
 */
export function bashExitCode(errorMessage: string): number | null {
  const m = /^Error: Exit code (\d+)/.exec(errorMessage);
  if (!m) return null;
  const code = Number.parseInt(m[1], 10);
  return Number.isFinite(code) ? code : null;
}

// ── Derived helpers ──────────────────────────────────────────────────────────

/** Added / removed line counts across every hunk, for an `Edit`/`Write` header. */
export function diffStats(hunks: PatchHunk[]): { added: number; removed: number } {
  let added = 0;
  let removed = 0;
  for (const hunk of hunks) {
    for (const line of hunk.lines) {
      if (line.startsWith("+")) added++;
      else if (line.startsWith("-")) removed++;
    }
  }
  return { added, removed };
}

/**
 * Fleet's own decision card. The MCP tool name is namespaced by the server, so
 * match on the tail rather than hard-coding the full `mcp__fleet__fleet__ask`.
 */
export function isDecisionTool(toolName: string): boolean {
  return toolName === "AskUserQuestion" || toolName.endsWith("fleet__ask");
}

/**
 * Map every `tool_use_id` to the `toolUseResult` recorded alongside its result.
 *
 * The payload hangs off the *message* rather than the `tool_result` block, so
 * this join is only unambiguous because Claude Code emits exactly one
 * `tool_result` per message — verified across 11,112 result-bearing messages,
 * every one holding a single block, including parallel tool calls.
 */
export function buildToolResultMetaMap(messages: RawMessage[]): Map<string, unknown> {
  const map = new Map<string, unknown>();
  for (const msg of messages) {
    if (msg.toolUseResult === undefined) continue;
    const content = msg.message?.content;
    if (!Array.isArray(content)) continue;
    for (const block of content) {
      if (block.type !== "tool_result") continue;
      map.set((block as ToolResultBlock).tool_use_id, msg.toolUseResult);
    }
  }
  return map;
}
