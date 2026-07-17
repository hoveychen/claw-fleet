import { useState } from "react";
import { useTranslation } from "react-i18next";
import type {
  ContentBlock,
  ImageBlock,
  ToolResultBlock,
  ToolUseBlock as ToolUseBlockType,
} from "../../types";
import { asFileEditResult } from "../../toolResults";
import type { PathLinkContext } from "../../markdown/pathLinks";
import { DiffView } from "./DiffView";
import { ExpandableText } from "./ExpandableText";
import { ImageThumb } from "./ImageThumb";
import { AgentInput, ToolBody, groupLabel, hasCustomBody, headerStats } from "./toolPresenters";
import { useFullToolResult, useToolResultFetch } from "./toolResultFetch";
import styles from "./ToolUseBlock.module.css";

// Read-only tools that get grouped into a single summary row
const READ_ONLY_TOOLS = new Set([
  "Read",
  "Grep",
  "Glob",
  "WebSearch",
  "WebFetch",
  "TodoWrite",
  "TodoRead",
  "Explore",
]);

interface Props {
  block: ToolUseBlockType;
  result?: ToolResultBlock;
  isPartial?: boolean; // no result yet
  /** Claude Code's structured `toolUseResult` for this call, when recorded. */
  meta?: unknown;
  /** Workspace context; the collapsed row strips its root from path summaries.
   *  The expanded body keeps absolute paths — precision belongs there. */
  paths?: PathLinkContext;
}

interface MultiEditEdit {
  old_string: string;
  new_string: string;
  replace_all?: boolean;
}

/** `/Users/x/ws/app/foo.ts` → `app/foo.ts` when the workspace root is known. */
function relToWorkspace(p: string, root?: string): string {
  if (!root) return p;
  const prefix = root.endsWith("/") ? root : `${root}/`;
  return p.startsWith(prefix) ? p.slice(prefix.length) : p;
}

function formatInput(input: Record<string, unknown>, name?: string, wsRoot?: string): string {
  // Bash and Agent both carry a model-written `description` on every call — for
  // Bash a one-line summary ("Find files referencing meta block names"), for
  // Agent a 3-5 word task name. It reads far better in the collapsed row than
  // the alternatives: Bash's raw command is often a long escaped grep, and
  // Agent has no compact field so it used to fall through to JSON.stringify.
  // The full command / prompt still shows in the expanded body (BashBody /
  // AgentBody). Fall back when no description was recorded (older transcripts).
  if (name === "Bash" || name === "Agent") {
    const desc = typeof input.description === "string" ? input.description.trim() : "";
    if (desc) return desc;
  }
  // codex runs in "code mode": every tool call is an `exec` running a JS/TS
  // script with no per-call description. The injected AGENTS.md (Rule 7) asks
  // the model to put a `// ...` summary on the first line, which the backend
  // lifts into `exec_note`. Prefer it in the collapsed row so 老板 sees intent
  // instead of raw code; the full script still shows in the expanded body.
  // Fall back to the raw command when the summary is absent.
  if (name === "exec") {
    const note = typeof input.exec_note === "string" ? input.exec_note.trim() : "";
    if (note) return note;
  }
  // Show a compact one-liner for common tools
  if ("command" in input) return String(input.command);
  // codex's `exec_command` carries the shell command under `cmd` (not
  // `command`), so surface it the same way — otherwise the whole args object
  // ({cmd, workdir, yield_time_ms, max_output_tokens}) dumps as raw JSON.
  if ("cmd" in input) return String(input.cmd);
  if ("file_path" in input) return relToWorkspace(String(input.file_path), wsRoot);
  if ("pattern" in input) return String(input.pattern);
  if ("path" in input) return relToWorkspace(String(input.path), wsRoot);
  if ("query" in input) return String(input.query);
  if ("url" in input) return String(input.url);
  return JSON.stringify(input, null, 2);
}

/**
 * Coerce a millisecond timeout to a compact seconds string (30000 → "30",
 * 1500 → "1.5"), or null when it is absent / unusable. codex serialises these
 * fields inconsistently — `wait.yield_time_ms` arrives as a JSON number, while
 * `wait_agent.timeout_ms` / `exec_command.yield_time_ms` arrive as numeric
 * strings — so accept both shapes.
 */
export function timeoutMsToSecs(value: unknown): string | null {
  const ms = typeof value === "string" ? Number(value) : value;
  if (typeof ms !== "number" || !Number.isFinite(ms) || ms <= 0) return null;
  const s = ms / 1000;
  return Number.isInteger(s) ? String(s) : s.toFixed(1);
}

/** Back-compat alias: `wait`'s timeout lives in `yield_time_ms`. */
export function waitTimeoutSecs(input: Record<string, unknown>): string | null {
  return timeoutMsToSecs(input.yield_time_ms);
}

/** One file touched by a codex `apply_patch` hunk. */
interface PatchFile {
  op: "Add" | "Update" | "Delete";
  path: string;
}

/**
 * Pull the touched files out of a codex `apply_patch` body. The patch is the V4A
 * text format — file headers read `*** Add File: <path>` / `*** Update File:
 * <path>` / `*** Delete File: <path>` (a survey of 38 local rollouts showed
 * every apply_patch starting with `*** Begin Patch`, so the whole body would
 * otherwise collapse to that useless first line).
 */
export function parsePatchFiles(patch: string): PatchFile[] {
  const files: PatchFile[] = [];
  const re = /^\*\*\* (Add|Update|Delete) File: (.+)$/gm;
  let m: RegExpExecArray | null;
  while ((m = re.exec(patch)) !== null) {
    files.push({ op: m[1] as PatchFile["op"], path: m[2].trim() });
  }
  return files;
}

/** Turn a JS string literal (with its surrounding quotes) into its value.
 *  Double-quoted literals go through JSON.parse for correct unescaping; single
 *  and backtick literals get a minimal manual unescape. */
function unquoteJs(literal: string): string {
  const quote = literal[0];
  const inner = literal.slice(1, -1);
  if (quote === '"') {
    try {
      return JSON.parse(literal) as string;
    } catch {
      // Malformed escape — fall through to the manual pass below.
    }
  }
  return inner.replace(/\\(.)/g, (_, c: string) =>
    c === "n" ? "\n" : c === "t" ? "\t" : c === "r" ? "\r" : c,
  );
}

/**
 * Pull the real shell command (and workdir) out of a codex "code mode" exec
 * script. In code mode the `exec` tool's `command` is not a shell line but a JS
 * harness — `await tools.exec_command({ cmd: "…", workdir: "…", … })` — so the
 * command the model actually ran is buried in the `cmd` field. Returns `{}` when
 * `command` is not that shape (older-style exec, where `command` is already the
 * shell command, or any script with no `cmd:` field). String literals may be
 * double-, single-, or backtick-quoted; the first match of each key wins.
 */
export function parseExecCommand(command: string): { cmd?: string; workdir?: string } {
  const field = (key: string): string | undefined => {
    const m = command.match(
      new RegExp(`\\b${key}\\s*:\\s*("(?:[^"\\\\]|\\\\.)*"|'(?:[^'\\\\]|\\\\.)*'|\`[^\`]*\`)`),
    );
    return m ? unquoteJs(m[1]) : undefined;
  };
  return { cmd: field("cmd"), workdir: field("workdir") };
}

/**
 * Friendly one-liner for codex's function-call tools, which otherwise fall
 * through to a raw `JSON.stringify` of their args (a survey of 156 local
 * rollouts showed `wait`, `write_stdin`, `wait_agent`, `spawn_agent`,
 * `update_plan`, `request_user_input` all rendering as noise) — plus the
 * `apply_patch` custom-tool call, whose patch body otherwise shows only its
 * `*** Begin Patch` header. Returns null for anything not handled here so the
 * caller falls back to `formatInput`. The full args JSON still shows in the
 * expanded body — this only rewrites the collapsed summary. `t` is the i18next
 * translator; keys live under `detail.tool_*`.
 */
export function codexToolSummary(
  name: string,
  input: Record<string, unknown>,
  t: (key: string, opts?: Record<string, unknown>) => string,
  wsRoot?: string,
): string | null {
  const withTimeout = (base: string, timedKey: string, value: unknown) => {
    const secs = timeoutMsToSecs(value);
    return secs ? t(timedKey, { secs }) : t(base);
  };
  switch (name) {
    // Blocks on a background exec cell for more output; only the timeout reads.
    case "wait":
      return withTimeout("detail.tool_wait", "detail.tool_wait_timed", input.yield_time_ms);
    // Feeds stdin to a running session. Empty `chars` is a pure poll for more
    // output (same shape as `wait`); non-empty is real typed input.
    case "write_stdin": {
      const chars = typeof input.chars === "string" ? input.chars : "";
      if (chars.trim()) return t("detail.tool_stdin", { text: chars.trim() });
      return withTimeout("detail.tool_wait", "detail.tool_wait_timed", input.yield_time_ms);
    }
    // Waits for spawned subagent(s) to finish.
    case "wait_agent":
      return withTimeout(
        "detail.tool_wait_agent",
        "detail.tool_wait_agent_timed",
        input.timeout_ms,
      );
    // Spawns a subagent; label by its task name / type (the `message` is often
    // an opaque encrypted blob, so never surface it).
    case "spawn_agent": {
      const label =
        (typeof input.task_name === "string" && input.task_name.trim()) ||
        (typeof input.agent_type === "string" && input.agent_type.trim()) ||
        "";
      return label ? t("detail.tool_spawn_agent_named", { name: label }) : t("detail.tool_spawn_agent");
    }
    case "update_plan":
      return t("detail.tool_update_plan");
    case "request_user_input":
      return t("detail.tool_request_input");
    // A file edit delivered as a V4A patch (the backend puts the patch body
    // under `command`). Name the file(s) touched instead of dumping the patch.
    case "apply_patch": {
      const patch = typeof input.command === "string" ? input.command : "";
      const files = parsePatchFiles(patch);
      if (files.length === 0) return null; // let formatInput show the raw body
      if (files.length === 1) {
        const { op, path } = files[0];
        const key =
          op === "Add"
            ? "detail.tool_patch_add"
            : op === "Delete"
              ? "detail.tool_patch_delete"
              : "detail.tool_patch_update";
        return t(key, { path: relToWorkspace(path, wsRoot) });
      }
      return t("detail.tool_patch_multi", { count: files.length });
    }
    default:
      return null;
  }
}

/** True when an Agent call has a description or prompt worth rendering as
 *  text; if neither is present we keep the raw-JSON fallback. */
function hasAgentInput(block: ToolUseBlockType): boolean {
  return (
    typeof block.input.description === "string" || typeof block.input.prompt === "string"
  );
}

/**
 * Expanded body for a codex `exec` call. Instead of dumping the whole input as
 * raw JSON (which buries the real shell command inside an escaped JS harness
 * string), surface the `cmd` the model ran as the headline, its workdir
 * ws-relative underneath, and keep the full JS script available but folded. When
 * `command` isn't code-mode (no `cmd:` field) it IS the shell command, so show
 * it directly; when there's no `command` at all, fall back to the raw input.
 */
function ExecInput({ block, paths }: { block: ToolUseBlockType; paths?: PathLinkContext }) {
  const command = typeof block.input.command === "string" ? block.input.command : "";
  const { cmd, workdir } = command ? parseExecCommand(command) : {};

  if (cmd) {
    return (
      <>
        <span className={styles.section_label}>Command</span>
        <pre className={styles.input_text}>{cmd}</pre>
        {workdir && (
          <div className={styles.exec_workdir}>{relToWorkspace(workdir, paths?.workspaceRoot)}</div>
        )}
        <details className={styles.exec_script}>
          <summary className={styles.exec_script_summary}>Script</summary>
          <pre className={styles.input_text}>{command}</pre>
        </details>
      </>
    );
  }

  return (
    <>
      <span className={styles.section_label}>{command ? "Command" : "Input"}</span>
      <pre className={styles.input_text}>{command || JSON.stringify(block.input, null, 2)}</pre>
    </>
  );
}

/** Error colouring comes from `.result_error .result_text` on the wrapper. */
function ResultText({ text }: { text: string }) {
  return <ExpandableText text={text} className={styles.result_text} />;
}

/**
 * The blocks a `tool_result` can carry, rendered as themselves.
 *
 * Previously anything that wasn't a plain string went through
 * `JSON.stringify`, so a `Read` of an image file — 295 of them across 120 real
 * transcripts — showed up as two thousand characters of base64.
 */
function ResultBlocks({ blocks }: { blocks: ContentBlock[] }) {
  const { t } = useTranslation();
  const refs = blocks.filter((b) => b.type === "tool_reference");
  const rest = blocks.filter((b) => b.type !== "tool_reference");

  return (
    <>
      {/* ToolSearch answers with a list of tool names; a chip row beats JSON. */}
      {refs.length > 0 && (
        <div className={styles.tool_refs}>
          {refs.map((b, i) => (
            <span key={i} className={styles.tool_ref}>
              {String((b as { tool_name?: unknown }).tool_name ?? "?")}
            </span>
          ))}
        </div>
      )}
      {rest.map((block, i) => {
        if (block.type === "image") {
          return (
            <ImageThumb key={i} block={block as ImageBlock} alt={t("detail.result_image")} />
          );
        }
        if (block.type === "text") {
          return <ResultText key={i} text={(block as { text: string }).text} />;
        }
        // An unknown block still beats silence: show its shape.
        return <ResultText key={i} text={JSON.stringify(block, null, 2)} />;
      })}
    </>
  );
}

function ResultContent({ result }: { result: ToolResultBlock }) {
  const isError = !!result.is_error;

  return (
    <div className={`${styles.result} ${isError ? styles.result_error : ""}`}>
      {Array.isArray(result.content) ? (
        <ResultBlocks blocks={result.content} />
      ) : (
        <ResultText text={typeof result.content === "string" ? result.content : ""} />
      )}
    </div>
  );
}

/**
 * Render Edit/MultiEdit/Write input as a diff view; falls back to null.
 *
 * When Claude Code recorded a `structuredPatch` for the call, that wins: it is
 * the diff the tool actually applied, with true file line numbers. The
 * before/after strings remain the fallback for transcripts with no structured
 * payload.
 */
function DiffSection({ block, meta }: { block: ToolUseBlockType; meta?: unknown }) {
  const input = block.input;
  const edit = asFileEditResult(meta);
  const filePath =
    edit?.filePath ?? (typeof input.file_path === "string" ? input.file_path : undefined);

  if (block.name === "Edit") {
    const oldS = typeof input.old_string === "string" ? input.old_string : "";
    const newS = typeof input.new_string === "string" ? input.new_string : "";
    return (
      <DiffView
        filePath={filePath}
        hunks={edit?.structuredPatch}
        before={oldS}
        after={newS}
        tag="Edit"
      />
    );
  }

  if (block.name === "MultiEdit") {
    const edits = Array.isArray(input.edits) ? (input.edits as MultiEditEdit[]) : [];
    if (edits.length === 0) return null;
    return (
      <div className={styles.multiedit_stack}>
        {edits.map((e, i) => (
          <DiffView
            key={i}
            filePath={filePath}
            before={String(e.old_string ?? "")}
            after={String(e.new_string ?? "")}
            tag={`Edit ${i + 1}/${edits.length}`}
          />
        ))}
      </div>
    );
  }

  if (block.name === "Write") {
    const content = typeof input.content === "string" ? input.content : "";
    // With no structuredPatch and no recorded original, the prior content is
    // genuinely unknown: `before: null` renders content as a new-file diff.
    return (
      <DiffView
        filePath={filePath}
        hunks={edit?.structuredPatch}
        before={edit?.originalFile ?? null}
        after={content}
        tag="Write"
      />
    );
  }

  return null;
}

const DIFF_TOOLS = new Set(["Edit", "MultiEdit", "Write"]);

export function ToolUseBlock({ block, result: resultProp, isPartial, meta: metaProp, paths }: Props) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);

  // On expand, recover the full output if this card's tail payload was
  // truncated for transport (see the Rust `message_trim`). headerStats reads
  // only small fields the backend leaves intact, so the collapsed row never
  // needs this — the fetch is deferred until the reader actually opens the card.
  const toolFetch = useToolResultFetch();
  const wasTruncated = !!block.id && (toolFetch?.truncatedIds.has(block.id) ?? false);
  const { full, loadingFull } = useFullToolResult(open, wasTruncated, toolFetch, block.id);

  // Swap the recovered full body in for the truncated preview once it lands.
  const meta = full ? full.toolUseResult : metaProp;
  const result =
    full && resultProp ? { ...resultProp, content: full.content as ToolResultBlock["content"] } : resultProp;

  // codex's function-call tools get a friendly one-liner instead of their raw
  // args object; everything else falls back to the generic formatter.
  const summary =
    codexToolSummary(block.name, block.input, t, paths?.workspaceRoot) ??
    formatInput(block.input, block.name, paths?.workspaceRoot);
  const isReadOnly = READ_ONLY_TOOLS.has(block.name);
  const isDiffTool = DIFF_TOOLS.has(block.name);
  const custom = hasCustomBody(block.name, meta, result);
  const stats = headerStats(block, meta, result);

  return (
    <div className={`${styles.root} ${isReadOnly ? styles.readonly : ""}`}>
      <button className={styles.header} onClick={() => setOpen((o) => !o)}>
        <span className={styles.arrow}>{open ? "▾" : "▸"}</span>
        <span className={styles.tool_name}>{block.name}</span>
        {!open && (
          <span className={styles.summary}>{summary}</span>
        )}
        {stats}
        {isPartial && !result && (
          <span className={styles.spinner}>⟳</span>
        )}
        {result?.is_error && !open && (
          <span className={styles.error_badge}>error</span>
        )}
        {result && !result.is_error && !open && !stats && (
          <span className={styles.ok_dot} />
        )}
      </button>

      {open && (
        <div className={styles.body}>
          {loadingFull && !full && (
            <div className={styles.pending}>Loading full output…</div>
          )}
          {/* A failed Edit/Write applied nothing — drawing its intended diff
              would claim a change that never landed. Show the error instead. */}
          {isDiffTool && !result?.is_error ? (
            <div className={styles.input_section}>
              <DiffSection block={block} meta={meta} />
            </div>
          ) : custom ? (
            <div className={styles.input_section}>
              <ToolBody block={block} meta={meta} result={result} />
            </div>
          ) : block.name === "Agent" && hasAgentInput(block) ? (
            // A running (or non-Claude) subagent has no `toolUseResult` yet, so
            // the custom body can't render — but its description + prompt are
            // right here in the input. Show them readably instead of raw JSON.
            <div className={styles.input_section}>
              <AgentInput block={block} paths={paths} />
            </div>
          ) : block.name === "exec" ? (
            // codex `exec` has no `toolUseResult`, so it never hits the custom
            // body path — surface the real shell command from its JS harness
            // instead of the raw JSON input object.
            <div className={styles.input_section}>
              <ExecInput block={block} paths={paths} />
            </div>
          ) : (
            <div className={styles.input_section}>
              <span className={styles.section_label}>Input</span>
              <pre className={styles.input_text}>
                {JSON.stringify(block.input, null, 2)}
              </pre>
            </div>
          )}
          {/* A custom body already presents the result (stdout, todo list,
              subagent output); repeating the raw blob under it is noise. */}
          {result && !custom && <ResultContent result={result} />}
          {isPartial && !result && (
            <div className={styles.pending}>Running…</div>
          )}
        </div>
      )}
    </div>
  );
}

// ── Grouped read-only tools ───────────────────────────────────────────────────

interface GroupedProps {
  blocks: Array<{ block: ToolUseBlockType; result?: ToolResultBlock; meta?: unknown }>;
  paths?: PathLinkContext;
}

export function GroupedToolUseBlocks({ blocks, paths }: GroupedProps) {
  const [open, setOpen] = useState(false);
  // The label used to read "Explored N files" for every group, including runs
  // of WebSearch or TodoWrite, which touch no files at all.
  const label = groupLabel(blocks.map((b) => b.block.name));

  return (
    <div className={styles.group}>
      <button className={styles.group_toggle} onClick={() => setOpen((o) => !o)}>
        <span className={styles.arrow}>{open ? "▾" : "▸"}</span>
        <span className={styles.group_label}>{label}</span>
      </button>
      {open && (
        <div className={styles.group_body}>
          {blocks.map(({ block, result, meta }, i) => (
            <ToolUseBlock key={block.id ?? i} block={block} result={result} meta={meta} paths={paths} />
          ))}
        </div>
      )}
    </div>
  );
}
