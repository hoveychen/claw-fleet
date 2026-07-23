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
import { fileExtIcon } from "./Rail";
import { ExpandableText } from "./ExpandableText";
import { ImageThumb } from "./ImageThumb";
import { resultHasTrimmedImage } from "../../imageData";
import { TextBlock } from "./TextBlock";
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
  /** Rendered as a work-run rail step: the gutter icon already names the tool,
   *  so the row drops its card chrome — no border, no tinted header, no
   *  expand arrow, no tool-name label, no success dot. Just the summary line
   *  (tool name available on hover via title) over the same expandable body. */
  rail?: boolean;
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

/** `/Users/x/ws/app/foo.ts` → `foo.ts`. The collapsed row only needs to name
 *  which file a tool touched; the full absolute path lives in the expanded
 *  body (DiffView path bar / ReadInput headline). Relativizing to the
 *  workspace root wasn't enough — a file nested under `.worktrees/…` still
 *  rendered a long, ellipsis-truncated path. */
export function basename(p: string): string {
  const parts = p.split(/[/\\]/);
  return parts[parts.length - 1] || p;
}

function formatInput(input: Record<string, unknown>, name?: string): string {
  // Bash and Agent both carry a model-written `description` on every call — for
  // Bash a one-line summary ("Find files referencing meta block names"), for
  // Agent a 3-5 word task name. It reads far better in the collapsed row than
  // the alternatives: Bash's raw command is often a long escaped grep, and
  // Agent has no compact field so it used to fall through to JSON.stringify.
  // The full command / prompt still shows in the expanded body (BashBody /
  // AgentBody). Fall back when no description was recorded (older transcripts).
  // PowerShell is the Windows shell tool (same `description`/`command` shape as
  // Bash); treat it identically so the collapsed row shows the summary line.
  if (name === "Bash" || name === "PowerShell" || name === "Agent") {
    const desc = typeof input.description === "string" ? input.description.trim() : "";
    if (desc) return desc;
  }
  // codex "code mode" execs (`exec`) are summarised in `codexToolSummary` — it
  // prefers the lifted `exec_note` and falls back to the wrapper's `cmd`. Only
  // when both are absent does control reach here, dumping the raw script below.
  // Show a compact one-liner for common tools
  if ("command" in input) return String(input.command);
  // codex's `exec_command` carries the shell command under `cmd` (not
  // `command`), so surface it the same way — otherwise the whole args object
  // ({cmd, workdir, yield_time_ms, max_output_tokens}) dumps as raw JSON.
  if ("cmd" in input) return String(input.cmd);
  if ("file_path" in input) return basename(String(input.file_path));
  // NotebookEdit names its target `notebook_path`, not `file_path` — surface it
  // the same way so it reads as a filename instead of a raw args dump.
  if ("notebook_path" in input) return basename(String(input.notebook_path));
  if ("pattern" in input) return String(input.pattern);
  if ("path" in input) return basename(String(input.path));
  if ("query" in input) return String(input.query);
  if ("url" in input) return String(input.url);
  // Last resort (unhandled MCP tools, etc.): a compact single-line object, not a
  // multi-line pretty-print — the collapsed row is a one-line flex cell, so
  // indentation just renders as a ragged blob. The expanded body (ParamsBody)
  // still shows the full labeled key/value list.
  return JSON.stringify(input);
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
  movePath?: string;
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
  for (const line of patch.split("\n")) {
    const header = line.match(/^\*\*\* (Add|Update|Delete) File: (.+)$/);
    if (header) {
      files.push({ op: header[1] as PatchFile["op"], path: header[2].trim() });
      continue;
    }
    const move = line.match(/^\*\*\* Move to: (.+)$/);
    if (move && files.length > 0) {
      files[files.length - 1].movePath = move[1].trim();
    }
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
 * double-, single-, or backtick-quoted; the first match of each key wins. The
 * key itself may be bare (`cmd:`, JS object literal) or quoted (`"cmd":`,
 * `'cmd':` — codex serialises the wrapper as JSON, so real rollouts use the
 * quoted form); both are accepted.
 */
export function parseExecCommand(command: string): { cmd?: string; workdir?: string } {
  const field = (key: string): string | undefined => {
    const m = command.match(
      new RegExp(`["'\`]?\\b${key}\\b["'\`]?\\s*:\\s*("(?:[^"\\\\]|\\\\.)*"|'(?:[^'\\\\]|\\\\.)*'|\`[^\`]*\`)`),
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
    // codex "code mode": the tool call is a JS harness. The injected AGENTS.md
    // (Rule 7) asks the model to put a `// ...` summary on the first line, which
    // the backend lifts into `exec_note` — prefer it so the collapsed row shows
    // intent. When the model skips the comment, fall back to the real shell
    // command buried in the `tools.exec_command({ cmd })` wrapper, so the row
    // reads `sed -n …` rather than the raw `const r = await …` JS. Only when
    // neither exists (e.g. an `exec` calling `tools.view_image`) do we return
    // null and let formatInput dump the raw script.
    case "exec": {
      const note = typeof input.exec_note === "string" ? input.exec_note.trim() : "";
      if (note) return note;
      const command = typeof input.command === "string" ? input.command : "";
      const { cmd } = parseExecCommand(command);
      return cmd ?? null;
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
        return t(key, { path: basename(path) });
      }
      return t("detail.tool_patch_multi", { count: files.length });
    }
    default:
      return null;
  }
}

/**
 * Collapsed one-liner for Claude Code built-in tools whose input carries none
 * of formatInput's recognised keys (command/cmd/file_path/pattern/path/query/
 * url) and so would otherwise dump their raw `{ … }` object into the row (the
 * blob 老板 flagged for `Skill`). Sits between codexToolSummary and formatInput
 * in the summary chain; returns null for anything it doesn't handle so control
 * falls through.
 *
 * Note some of these tools already have a dedicated *expanded* body
 * (ExitPlanMode → PlanModeInput, Skill → SkillInput) — this only fixes the
 * *collapsed* row, which was never wired up. The SKILL.md body itself is a
 * separate meta turn (MetaFoldBlock's "已加载 SKILL" card), not this call.
 */
export function claudeToolSummary(
  name: string,
  input: Record<string, unknown>,
  t: (key: string, opts?: Record<string, unknown>) => string,
): string | null {
  switch (name) {
    // { skill: "game-pilot", args?: "…" }
    case "Skill": {
      const slug = typeof input.skill === "string" ? input.skill.trim() : "";
      return slug ? t("detail.tool_skill_named", { slug }) : t("detail.tool_skill");
    }
    // { plan: "<markdown>" } — the plan renders in the expanded PlanModeInput.
    case "ExitPlanMode":
      return t("detail.tool_exit_plan");
    // { todos: [...] }. Standalone calls reach here; grouped runs get their own
    // "Updated the plan" label from GroupedToolUseBlocks.
    case "TodoWrite": {
      const todos = Array.isArray(input.todos) ? input.todos : null;
      return todos ? t("detail.tool_todo_count", { count: todos.length }) : t("detail.tool_todo");
    }
    // { bash_id, filter? } — polls a background shell's output.
    case "BashOutput":
      return t("detail.tool_bash_output");
    // { shell_id | bash_id } — kills a background shell (KillBash is the older name).
    case "KillShell":
    case "KillBash":
      return t("detail.tool_kill_shell");
    // { query, max_results } — loads deferred tool schemas. The raw query is
    // opaque to readers ("select:AskUserQuestion", "notebook jupyter", "+slack
    // send"); surface a readable label instead. "select:A,B" lists the tools
    // being loaded; anything else is a keyword search over deferred tools.
    case "ToolSearch": {
      const query = typeof input.query === "string" ? input.query.trim() : "";
      if (!query) return t("detail.tool_search");
      const sel = query.match(/^select:(.*)$/i);
      if (sel) {
        const names = sel[1]
          .split(",")
          .map((s) => s.trim())
          .filter(Boolean)
          .join(", ");
        return names ? t("detail.tool_search_load", { names }) : t("detail.tool_search");
      }
      return t("detail.tool_search_query", { query });
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
 * One input field as a labeled row. Scalars stay inline (`key  value`); long or
 * multiline strings break onto their own line with an ExpandableText (copy
 * button + height cap); scalar arrays become chips; nested objects fall to a
 * labeled JSON block. Keeps every field readable without ever dumping the whole
 * args object as one JSON blob.
 */
function ParamRow({ name, value }: { name: string; value: unknown }) {
  if (typeof value === "string") {
    if (value.includes("\n") || value.length > 80) {
      return (
        <div className={styles.param_block}>
          <span className={styles.param_key}>{name}</span>
          <ExpandableText text={value} />
        </div>
      );
    }
    return (
      <div className={styles.param_row}>
        <span className={styles.param_key}>{name}</span>
        <span className={styles.param_val}>{value}</span>
      </div>
    );
  }
  if (typeof value === "number" || typeof value === "boolean" || value === null) {
    return (
      <div className={styles.param_row}>
        <span className={styles.param_key}>{name}</span>
        <span className={styles.param_val}>{String(value)}</span>
      </div>
    );
  }
  if (Array.isArray(value) && value.every((v) => typeof v === "string" || typeof v === "number")) {
    return (
      <div className={styles.param_row}>
        <span className={styles.param_key}>{name}</span>
        <span className={styles.param_chips}>
          {value.map((v, i) => (
            <span key={i} className={styles.param_chip}>
              {String(v)}
            </span>
          ))}
        </span>
      </div>
    );
  }
  return (
    <div className={styles.param_block}>
      <span className={styles.param_key}>{name}</span>
      <ExpandableText text={JSON.stringify(value, null, 2)} />
    </div>
  );
}

/**
 * Generic fallback body: the tool input as a labeled key/value list rather than
 * a raw `JSON.stringify` blob. Every tool with no dedicated presenter — MCP
 * tools, NotebookEdit, BashOutput, KillShell, … — lands here, so nothing in the
 * transcript shows raw JSON anymore. An empty input reads as a short note.
 */
function ParamsBody({ block }: { block: ToolUseBlockType }) {
  const entries = Object.entries(block.input ?? {});
  if (entries.length === 0) return <div className={styles.codex_note}>No parameters</div>;
  return (
    <div className={styles.params}>
      {entries.map(([k, v]) => (
        <ParamRow key={k} name={k} value={v} />
      ))}
    </div>
  );
}

/** Back-compat alias: codex presenters fall back here when their shape doesn't
 *  match. Now a clean key/value list instead of raw JSON. */
function RawInput({ block }: { block: ToolUseBlockType }) {
  return <ParamsBody block={block} />;
}

/**
 * Expanded body for codex's two shell tools. `exec_command` carries the shell
 * command directly under `cmd` (+ `workdir`); `exec` runs in "code mode" where
 * `command` is a JS harness string and the real command is buried inside a
 * `tools.exec_command({ cmd, workdir })` call. Either way, surface the shell
 * command as the headline, its workdir ws-relative underneath, and — only when
 * there's a JS wrapper worth hiding — keep the full script folded. When no shell
 * command can be found (e.g. an `exec` calling `tools.view_image`) show the raw
 * script as a code block; with no `command` at all fall back to raw JSON.
 */
function ExecInput({ block, paths }: { block: ToolUseBlockType; paths?: PathLinkContext }) {
  const directCmd = typeof block.input.cmd === "string" ? block.input.cmd : "";
  const command = typeof block.input.command === "string" ? block.input.command : "";
  const parsed = command ? parseExecCommand(command) : {};
  const cmd = directCmd || parsed.cmd;
  const workdir =
    (typeof block.input.workdir === "string" ? block.input.workdir : undefined) ?? parsed.workdir;

  if (cmd) {
    // Fold the JS wrapper away only when it adds something beyond the command
    // itself (code mode); exec_command has no wrapper, so `command` is empty.
    const hasWrapper = command.trim() !== "" && command.trim() !== cmd.trim();
    return (
      <>
        <span className={styles.section_label}>Command</span>
        <pre className={styles.input_text}>{cmd}</pre>
        {workdir && (
          <div className={styles.exec_workdir}>{relToWorkspace(workdir, paths?.workspaceRoot)}</div>
        )}
        {hasWrapper && (
          <details className={styles.exec_script}>
            <summary className={styles.exec_script_summary}>Script</summary>
            <pre className={styles.input_text}>{command}</pre>
          </details>
        )}
      </>
    );
  }

  if (command) {
    return (
      <>
        <span className={styles.section_label}>Command</span>
        <pre className={styles.input_text}>{command}</pre>
      </>
    );
  }
  return <RawInput block={block} />;
}

/** Expanded body for codex `apply_patch`: a header row naming each touched file
 *  (op + ws-relative path) over the raw V4A patch text. The patch is already a
 *  readable diff format, so showing it verbatim beats the JSON-escaped blob it
 *  replaces. `ExpandableText` handles long patches and gives a copy button. */
function PatchInput({ block, paths }: { block: ToolUseBlockType; paths?: PathLinkContext }) {
  const patch = typeof block.input.command === "string" ? block.input.command : "";
  if (!patch) return <RawInput block={block} />;
  const files = parsePatchFiles(patch);
  return (
    <>
      {files.length > 0 && (
        <div className={styles.patch_files}>
          {files.map((f, i) => (
            <span key={i} className={styles.patch_file}>
              <span className={styles.patch_op} data-op={f.op}>
                {f.op}
              </span>
              {relToWorkspace(f.path, paths?.workspaceRoot)}
              {f.movePath && ` → ${relToWorkspace(f.movePath, paths?.workspaceRoot)}`}
            </span>
          ))}
        </div>
      )}
      <ExpandableText text={patch} />
    </>
  );
}

const PLAN_MARK: Record<string, string> = {
  completed: "☑",
  in_progress: "▶",
  pending: "☐",
};

interface PlanStep {
  step?: string;
  status?: string;
}

/** Expanded body for codex `update_plan`: the step list rendered as a checklist,
 *  mirroring Claude Code's TodoWrite body instead of dumping the plan array. */
function PlanInput({ block }: { block: ToolUseBlockType }) {
  const plan = Array.isArray(block.input.plan) ? (block.input.plan as PlanStep[]) : [];
  if (plan.length === 0) return <RawInput block={block} />;
  return (
    <ul className={styles.plan_list}>
      {plan.map((s, i) => (
        <li
          key={i}
          className={`${styles.plan_item} ${s.status === "completed" ? styles.plan_done : ""} ${
            s.status === "in_progress" ? styles.plan_current : ""
          }`}
        >
          <span className={styles.plan_mark} aria-hidden>
            {PLAN_MARK[s.status ?? ""] ?? "☐"}
          </span>
          <span className={styles.plan_text}>{s.step ?? ""}</span>
        </li>
      ))}
    </ul>
  );
}

/** Expanded body for codex `spawn_agent`: type/model/effort chips over the task
 *  message. The `message` can be an opaque encrypted blob (a long token with no
 *  whitespace) — show it only when it reads as prose. */
function SpawnAgentInput({ block }: { block: ToolUseBlockType }) {
  const i = block.input;
  const chips = [i.agent_type, i.model, i.reasoning_effort]
    .map((v) => (typeof v === "string" ? v.trim() : ""))
    .filter(Boolean);
  const message = typeof i.message === "string" ? i.message : "";
  const isProse = /\s/.test(message.trim());
  if (chips.length === 0 && !isProse) return <RawInput block={block} />;
  return (
    <>
      {chips.length > 0 && (
        <div className={styles.codex_chips}>
          {chips.map((c, k) => (
            <span key={k} className={styles.codex_chip}>
              {c}
            </span>
          ))}
        </div>
      )}
      {isProse && <ExpandableText text={message} />}
    </>
  );
}

interface CodexQuestion {
  header?: string;
  question?: string;
  options?: Array<{ label?: string; description?: string }>;
}

/** Expanded body for codex `request_user_input`: each question over its options,
 *  instead of the raw `{questions:[…]}` array. */
function RequestInput({ block }: { block: ToolUseBlockType }) {
  const questions = Array.isArray(block.input.questions)
    ? (block.input.questions as CodexQuestion[])
    : [];
  if (questions.length === 0) return <RawInput block={block} />;
  return (
    <div className={styles.request_list}>
      {questions.map((q, i) => (
        <div key={i} className={styles.request_q}>
          <div className={styles.request_q_text}>{q.question ?? q.header ?? ""}</div>
          {Array.isArray(q.options) && q.options.length > 0 && (
            <ul className={styles.request_opts}>
              {q.options.map((o, k) => (
                <li key={k}>
                  {o.label}
                  {o.description ? ` — ${o.description}` : ""}
                </li>
              ))}
            </ul>
          )}
        </div>
      ))}
    </div>
  );
}

/** Expanded body for codex `write_stdin`: the typed characters, or a note when
 *  it's an empty poll for more output. */
function StdinInput({ block }: { block: ToolUseBlockType }) {
  const chars = typeof block.input.chars === "string" ? block.input.chars : "";
  if (!chars.trim()) {
    const secs = waitTimeoutSecs(block.input);
    return (
      <div className={styles.codex_note}>{secs ? `Polled for output · ${secs}s` : "Polled for output"}</div>
    );
  }
  return (
    <>
      <span className={styles.section_label}>Input</span>
      <pre className={styles.input_text}>{chars}</pre>
    </>
  );
}

/** Expanded body for codex `wait` / `wait_agent`: a compact note (timeout, cell,
 *  agent count) rather than the raw field object. */
function WaitInput({ block }: { block: ToolUseBlockType }) {
  const secs = timeoutMsToSecs(block.input.yield_time_ms ?? block.input.timeout_ms);
  const cell = typeof block.input.cell_id === "string" ? block.input.cell_id : "";
  const ids = Array.isArray(block.input.ids) ? (block.input.ids as string[]) : [];
  const parts: string[] = [];
  if (secs) parts.push(`timeout ${secs}s`);
  if (cell) parts.push(`cell ${cell}`);
  if (ids.length) parts.push(`${ids.length} agent${ids.length === 1 ? "" : "s"}`);
  if (parts.length === 0) return <RawInput block={block} />;
  return <div className={styles.codex_note}>{parts.join(" · ")}</div>;
}

/** codex's tool set (function-call + custom tools). These have no Claude Code
 *  `toolUseResult`, so they never hit the custom-body path — their expanded body
 *  is dispatched through `CodexToolInput` instead of falling to raw JSON. */
const CODEX_TOOLS = new Set([
  "exec",
  "exec_command",
  "apply_patch",
  "update_plan",
  "spawn_agent",
  "request_user_input",
  "wait",
  "wait_agent",
  "write_stdin",
]);

/** Routes each codex tool to its expanded-body presenter; unhandled tools fall
 *  back to raw JSON. */
function CodexToolInput({ block, paths }: { block: ToolUseBlockType; paths?: PathLinkContext }) {
  switch (block.name) {
    case "exec":
    case "exec_command":
      return <ExecInput block={block} paths={paths} />;
    case "apply_patch":
      return <PatchInput block={block} paths={paths} />;
    case "update_plan":
      return <PlanInput block={block} />;
    case "spawn_agent":
      return <SpawnAgentInput block={block} />;
    case "request_user_input":
      return <RequestInput block={block} />;
    case "write_stdin":
      return <StdinInput block={block} />;
    case "wait":
    case "wait_agent":
      return <WaitInput block={block} />;
    default:
      return <RawInput block={block} />;
  }
}

// ── Claude read-only / plan tools ─────────────────────────────────────────────

/**
 * Compact line-range note for a `Read` call's `offset`/`limit`: "lines 40–80",
 * "from line 40", or "first 80 lines". null when the whole file was read (no
 * offset and no limit), so the header shows just the path.
 */
export function readRange(offset: unknown, limit: unknown): string | null {
  const o = typeof offset === "number" && Number.isFinite(offset) ? offset : undefined;
  const l = typeof limit === "number" && Number.isFinite(limit) ? limit : undefined;
  if (o !== undefined && l !== undefined) return `lines ${o}–${o + l - 1}`;
  if (o !== undefined) return `from line ${o}`;
  if (l !== undefined) return `first ${l} lines`;
  return null;
}

/** Expanded body for `Read`: the file path as the headline with an optional
 *  line-range note. The collapsed row already names the file by basename, so
 *  the expanded headline shows the full absolute path — precision belongs here.
 *  The file content itself renders below as the tool result. */
function ReadInput({ block }: { block: ToolUseBlockType }) {
  const fp = typeof block.input.file_path === "string" ? block.input.file_path : "";
  if (!fp) return <ParamsBody block={block} />;
  const range = readRange(block.input.offset, block.input.limit);
  return (
    <div className={styles.target_head}>
      <span className={styles.target_main}>{fp}</span>
      {range && <span className={styles.target_note}>{range}</span>}
    </div>
  );
}

/**
 * The non-empty scalar flags of a `Grep`/`Glob` call as chip labels — search
 * root (ws-relative), `glob`, `type`, output mode, `-i`, `-n`. Skips the pattern
 * itself (shown as the headline) and any absent/empty field.
 */
export function searchFlags(input: Record<string, unknown>, wsRoot?: string): string[] {
  const chips: string[] = [];
  const path = typeof input.path === "string" ? input.path.trim() : "";
  if (path) chips.push(relToWorkspace(path, wsRoot));
  const glob = typeof input.glob === "string" ? input.glob.trim() : "";
  if (glob) chips.push(`glob ${glob}`);
  const type = typeof input.type === "string" ? input.type.trim() : "";
  if (type) chips.push(`type ${type}`);
  const mode = typeof input.output_mode === "string" ? input.output_mode.trim() : "";
  if (mode) chips.push(mode);
  if (input["-i"] === true) chips.push("-i");
  if (input["-n"] === true) chips.push("-n");
  return chips;
}

/** Expanded body for `Grep`: the regex as a code headline over its flag chips.
 *  Matches render below as the tool result. */
function GrepInput({ block, paths }: { block: ToolUseBlockType; paths?: PathLinkContext }) {
  const pattern = typeof block.input.pattern === "string" ? block.input.pattern : "";
  if (!pattern) return <ParamsBody block={block} />;
  const chips = searchFlags(block.input, paths?.workspaceRoot);
  return (
    <>
      <pre className={styles.input_text}>{pattern}</pre>
      {chips.length > 0 && (
        <div className={styles.param_chips}>
          {chips.map((c, i) => (
            <span key={i} className={styles.param_chip}>
              {c}
            </span>
          ))}
        </div>
      )}
    </>
  );
}

/** Expanded body for `Glob`: the pattern as the headline with an optional
 *  search-root note. Matched files render below as the tool result. */
function GlobInput({ block, paths }: { block: ToolUseBlockType; paths?: PathLinkContext }) {
  const pattern = typeof block.input.pattern === "string" ? block.input.pattern : "";
  if (!pattern) return <ParamsBody block={block} />;
  const path = typeof block.input.path === "string" ? block.input.path.trim() : "";
  return (
    <div className={styles.target_head}>
      <span className={styles.target_main}>{pattern}</span>
      {path && <span className={styles.target_note}>{relToWorkspace(path, paths?.workspaceRoot)}</span>}
    </div>
  );
}

/** Expanded body for `WebSearch`: the query as the headline over any allowed /
 *  blocked domain chips (`+example.com` / `−spam.com`). */
function WebSearchInput({ block }: { block: ToolUseBlockType }) {
  const query = typeof block.input.query === "string" ? block.input.query : "";
  if (!query) return <ParamsBody block={block} />;
  const allow = Array.isArray(block.input.allowed_domains) ? block.input.allowed_domains : [];
  const deny = Array.isArray(block.input.blocked_domains) ? block.input.blocked_domains : [];
  const chips = [
    ...allow.filter((d): d is string => typeof d === "string").map((d) => `+${d}`),
    ...deny.filter((d): d is string => typeof d === "string").map((d) => `−${d}`),
  ];
  return (
    <>
      <div className={styles.target_head}>
        <span className={styles.target_main}>{query}</span>
      </div>
      {chips.length > 0 && (
        <div className={styles.param_chips}>
          {chips.map((c, i) => (
            <span key={i} className={styles.param_chip}>
              {c}
            </span>
          ))}
        </div>
      )}
    </>
  );
}

/** Expanded body for `WebFetch`: the URL as the headline with the model prompt
 *  rendered as Markdown below it. */
function WebFetchInput({ block, paths }: { block: ToolUseBlockType; paths?: PathLinkContext }) {
  const url = typeof block.input.url === "string" ? block.input.url : "";
  const prompt = typeof block.input.prompt === "string" ? block.input.prompt : "";
  if (!url && !prompt) return <ParamsBody block={block} />;
  return (
    <>
      {url && (
        <div className={styles.target_head}>
          <span className={styles.target_main}>{url}</span>
        </div>
      )}
      {prompt && (
        <div className={styles.param_block}>
          <span className={styles.section_label}>Prompt</span>
          <TextBlock text={prompt} paths={paths} />
        </div>
      )}
    </>
  );
}

/** Expanded body for `ExitPlanMode`: the plan rendered as Markdown (headings,
 *  lists, code) instead of a single JSON-escaped string. */
function PlanModeInput({ block, paths }: { block: ToolUseBlockType; paths?: PathLinkContext }) {
  const plan = typeof block.input.plan === "string" ? block.input.plan : "";
  if (!plan) return <ParamsBody block={block} />;
  return <TextBlock text={plan} paths={paths} />;
}

/** Expanded body for `Skill`: the skill slug as the headline with its optional
 *  args rendered below (as Markdown when a string, else pretty JSON). The
 *  injected SKILL.md body is a separate meta turn (MetaFoldBlock), so this card
 *  only shows what was invoked, not the loaded instructions. */
function SkillInput({ block, paths }: { block: ToolUseBlockType; paths?: PathLinkContext }) {
  const slug = typeof block.input.skill === "string" ? block.input.skill : "";
  const argsVal = block.input.args;
  const argsText =
    typeof argsVal === "string"
      ? argsVal
      : argsVal == null
        ? ""
        : JSON.stringify(argsVal, null, 2);
  if (!slug && !argsText) return <ParamsBody block={block} />;
  return (
    <>
      {slug && (
        <div className={styles.target_head}>
          <span className={styles.target_main}>{slug}</span>
        </div>
      )}
      {argsText && (
        <div className={styles.param_block}>
          <span className={styles.section_label}>Args</span>
          <TextBlock text={argsText} paths={paths} />
        </div>
      )}
    </>
  );
}

/** Claude's read-only / plan tools that otherwise fall through to the generic
 *  param list. Each gets a headline + chips presenter instead. */
const CLAUDE_TOOLS = new Set([
  "Read",
  "Grep",
  "Glob",
  "WebSearch",
  "WebFetch",
  "ExitPlanMode",
  "Skill",
]);

/** Routes each Claude tool to its expanded-body presenter; unhandled names fall
 *  back to the generic param list. */
function ClaudeToolInput({ block, paths }: { block: ToolUseBlockType; paths?: PathLinkContext }) {
  switch (block.name) {
    case "Read":
      return <ReadInput block={block} />;
    case "Grep":
      return <GrepInput block={block} paths={paths} />;
    case "Glob":
      return <GlobInput block={block} paths={paths} />;
    case "WebSearch":
      return <WebSearchInput block={block} />;
    case "WebFetch":
      return <WebFetchInput block={block} paths={paths} />;
    case "ExitPlanMode":
      return <PlanModeInput block={block} paths={paths} />;
    case "Skill":
      return <SkillInput block={block} paths={paths} />;
    default:
      return <ParamsBody block={block} />;
  }
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

export function ToolUseBlock({ block, result: resultProp, isPartial, meta: metaProp, paths, rail }: Props) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);

  // On expand, recover the full output if this card's tail payload was
  // truncated for transport (see the Rust `message_trim`). headerStats reads
  // only small fields the backend leaves intact, so the collapsed row never
  // needs this — the fetch is deferred until the reader actually opens the card.
  const toolFetch = useToolResultFetch();
  const wasTruncated = !!block.id && (toolFetch?.truncatedIds.has(block.id) ?? false);
  // A completed Read may be present in the rendered window without its adjacent
  // tool_result (for example after live-tail/window reconciliation). That is
  // exactly the empty expanded card shape: use the transcript path to recover
  // the result lazily, just as we do for an explicitly truncated result.
  const missingReadResult = block.name === "Read" && !resultProp && !isPartial;
  // Self-heal: the trimmed base64 itself carries the trim marker, so a card
  // whose image data is a transport preview refetches on expand even when the
  // message-level `_fleetTruncated` flag was lost along the way. Without this,
  // a dropped flag feeds the undecodable preview straight to the <img>.
  const trimmedImageResult = resultHasTrimmedImage(resultProp);
  const { full, loadingFull, error: refetchError } = useFullToolResult(
    open,
    wasTruncated || missingReadResult || trimmedImageResult,
    toolFetch,
    block.id,
  );

  // Swap the recovered full body in for the truncated preview once it lands.
  const meta = full ? full.toolUseResult : metaProp;
  const result = full
    ? resultProp
      ? { ...resultProp, content: full.content as ToolResultBlock["content"] }
      : {
          type: "tool_result" as const,
          tool_use_id: block.id ?? "",
          content: full.content as ToolResultBlock["content"],
        }
    : resultProp;

  // codex's function-call tools get a friendly one-liner instead of their raw
  // args object; everything else falls back to the generic formatter.
  const summary =
    codexToolSummary(block.name, block.input, t) ??
    claudeToolSummary(block.name, block.input, t) ??
    formatInput(block.input, block.name);
  const isReadOnly = READ_ONLY_TOOLS.has(block.name);
  const isDiffTool = DIFF_TOOLS.has(block.name);
  const custom = hasCustomBody(block.name, meta, result);
  const stats = headerStats(block, meta, result);
  // For file tools (Edit/Read/Write/MultiEdit/NotebookEdit) the summary IS a
  // filename, so prepend a small glyph keyed on its extension. Gated on
  // `file_path` so Grep/Glob — whose summary is a pattern, not a file — don't
  // get a misleading file icon.
  const fileIconPath =
    typeof block.input.file_path === "string" ? block.input.file_path : undefined;

  return (
    <div className={`${styles.root} ${isReadOnly ? styles.readonly : ""} ${rail ? styles.rail : ""}`}>
      <button
        className={styles.header}
        onClick={() => setOpen((o) => !o)}
        title={rail ? block.name : undefined}
      >
        {!rail && <span className={styles.arrow}>{open ? "▾" : "▸"}</span>}
        {!rail && <span className={styles.tool_name}>{block.name}</span>}
        {(rail || !open) && fileIconPath && (
          <span className={styles.file_icon} aria-hidden>
            {fileExtIcon(fileIconPath)}
          </span>
        )}
        {(rail || !open) && (
          <span className={styles.summary}>{summary}</span>
        )}
        {stats}
        {isPartial && !result && (
          <span className={styles.spinner}>⟳</span>
        )}
        {result?.is_error && !open && (
          <span className={styles.error_badge}>error</span>
        )}
        {!rail && result && !result.is_error && !open && !stats && (
          <span className={styles.ok_dot} />
        )}
      </button>

      {open && (
        <div className={styles.body}>
          {loadingFull && !full && (
            <div className={styles.pending}>Loading full output…</div>
          )}
          {refetchError && (
            <div className={styles.pending}>Could not load full output: {refetchError}</div>
          )}
          {/* A failed Edit/Write applied nothing — drawing its intended diff
              would claim a change that never landed. Show the error instead. */}
          {isDiffTool && !result?.is_error ? (
            <div className={styles.input_section}>
              <DiffSection block={block} meta={meta} />
            </div>
          ) : custom ? (
            <div className={styles.input_section}>
              <ToolBody block={block} meta={meta} result={result} rail={rail} />
            </div>
          ) : block.name === "Agent" && hasAgentInput(block) ? (
            // A running (or non-Claude) subagent has no `toolUseResult` yet, so
            // the custom body can't render — but its description + prompt are
            // right here in the input. Show them readably instead of raw JSON.
            <div className={styles.input_section}>
              <AgentInput block={block} paths={paths} />
            </div>
          ) : CODEX_TOOLS.has(block.name) ? (
            // codex tools have no `toolUseResult`, so they never hit the custom
            // body path — dispatch each to a readable presenter instead of
            // dumping the raw JSON input object.
            <div className={styles.input_section}>
              <CodexToolInput block={block} paths={paths} />
            </div>
          ) : CLAUDE_TOOLS.has(block.name) ? (
            // Claude's read-only / plan tools: a headline (path, pattern, query,
            // url) + chips instead of the raw JSON input object.
            <div className={styles.input_section}>
              <ClaudeToolInput block={block} paths={paths} />
            </div>
          ) : (
            <div className={styles.input_section}>
              <ParamsBody block={block} />
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
  /** Same de-chroming as `ToolUseBlock`'s rail prop; member cards inherit it. */
  rail?: boolean;
}

export function GroupedToolUseBlocks({ blocks, paths, rail }: GroupedProps) {
  const [open, setOpen] = useState(false);
  // The label used to read "Explored N files" for every group, including runs
  // of WebSearch or TodoWrite, which touch no files at all.
  const label = groupLabel(blocks.map((b) => b.block.name));

  return (
    <div className={`${styles.group} ${rail ? styles.rail_group : ""}`}>
      <button className={styles.group_toggle} onClick={() => setOpen((o) => !o)}>
        {!rail && <span className={styles.arrow}>{open ? "▾" : "▸"}</span>}
        <span className={styles.group_label}>{label}</span>
      </button>
      {open && (
        <div className={styles.group_body}>
          {blocks.map(({ block, result, meta }, i) => (
            <ToolUseBlock key={block.id ?? i} block={block} result={result} meta={meta} paths={paths} rail={rail} />
          ))}
        </div>
      )}
    </div>
  );
}
