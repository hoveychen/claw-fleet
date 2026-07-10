/**
 * Per-tool rendering for the transcript's tool cards.
 *
 * Every tool used to collapse to `▸ Name <one-line input>` and expand to raw
 * JSON plus a truncated `<pre>` of the result. Claude Code already records far
 * more than that in `toolUseResult` (see `toolResults.ts`), so this module
 * turns each tool's payload into the header stats and body it deserves.
 *
 * Everything here is optional: `headerStats` returns null and `ToolBody`
 * returns null when a tool has no presenter or its payload doesn't match. The
 * caller falls back to the generic rendering, which is what keeps non-Claude
 * agent sources (which emit no `toolUseResult`) working unchanged.
 */

import { useState, type ReactNode } from "react";
import {
  asAgentResult,
  asBashResult,
  asErrorMessage,
  asFileEditResult,
  asFileSearchResult,
  asTodoWriteResult,
  bashExitCode,
  diffStats,
} from "../../toolResults";
import type { ToolResultBlock, ToolUseBlock as ToolUseBlockType } from "../../types";
import styles from "./toolPresenters.module.css";

/**
 * Tools whose body this module renders itself. Edit/Write are absent by design:
 * their body is a diff, owned by the caller's `DiffView`.
 */
const CUSTOM_BODY = new Set(["Bash", "Agent", "TodoWrite"]);

/**
 * Whether `ToolBody` will render something for this call. A missing payload or
 * a failed call falls back to the generic body, which shows the raw error.
 */
export function hasCustomBody(
  toolName: string,
  meta: unknown,
  result?: ToolResultBlock,
): boolean {
  return CUSTOM_BODY.has(toolName) && meta !== undefined && !result?.is_error;
}

function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`;
  const m = Math.floor(ms / 60_000);
  const s = Math.round((ms % 60_000) / 1000);
  return `${m}m${s.toString().padStart(2, "0")}s`;
}

function formatTokens(n: number): string {
  return n >= 1000 ? `${(n / 1000).toFixed(1)}k` : String(n);
}

// ── Header stats ─────────────────────────────────────────────────────────────

/**
 * Small chips shown on the collapsed header — the numbers worth seeing without
 * opening the card.
 */
export function headerStats(
  block: ToolUseBlockType,
  meta: unknown,
  result?: ToolResultBlock,
): ReactNode {
  if (meta === undefined) return null;

  if (block.name === "Bash") {
    // Bash records no exit code on success; a failure spells it out in the
    // error string, which is the only place it appears at all.
    if (result?.is_error) {
      const code = bashExitCode(asErrorMessage(meta) ?? "");
      return code === null ? null : <span className={styles.chip_err}>exit {code}</span>;
    }
    const bash = asBashResult(meta);
    if (!bash) return null;
    return (
      <>
        {bash.interrupted && <span className={styles.chip_warn}>interrupted</span>}
        {bash.stderr.trim() !== "" && <span className={styles.chip_warn}>stderr</span>}
      </>
    );
  }

  if (block.name === "Edit" || block.name === "Write") {
    const edit = asFileEditResult(meta);
    if (!edit) return null;
    const { added, removed } = diffStats(edit.structuredPatch);
    return (
      <>
        {added > 0 && <span className={styles.chip_add}>+{added}</span>}
        {removed > 0 && <span className={styles.chip_del}>−{removed}</span>}
      </>
    );
  }

  if (block.name === "Agent") {
    const agent = asAgentResult(meta);
    if (!agent) return null;
    return (
      <>
        {agent.agentType && <span className={styles.chip}>{agent.agentType}</span>}
        {agent.totalDurationMs !== undefined && (
          <span className={styles.chip}>{formatDuration(agent.totalDurationMs)}</span>
        )}
        {agent.totalTokens !== undefined && (
          <span className={styles.chip}>{formatTokens(agent.totalTokens)} tok</span>
        )}
      </>
    );
  }

  if (block.name === "Glob" || block.name === "Grep") {
    const search = asFileSearchResult(meta);
    if (!search) return null;
    const n = search.numMatches ?? search.numFiles;
    const unit = block.name === "Grep" ? "matches" : "files";
    return (
      <>
        <span className={styles.chip}>{n} {unit}</span>
        {search.truncated && <span className={styles.chip_warn}>truncated</span>}
      </>
    );
  }

  if (block.name === "TodoWrite") {
    const todo = asTodoWriteResult(meta);
    if (!todo) return null;
    const done = todo.newTodos.filter((it) => it.status === "completed").length;
    return <span className={styles.chip}>{done}/{todo.newTodos.length}</span>;
  }

  return null;
}

// ── Bodies ───────────────────────────────────────────────────────────────────

const MAX_STREAM_CHARS = 4000;

function Stream({ label, text, error }: { label: string; text: string; error?: boolean }) {
  const truncated = text.length > MAX_STREAM_CHARS;
  return (
    <div className={styles.stream}>
      <span className={`${styles.stream_label} ${error ? styles.stream_label_err : ""}`}>
        {label}
      </span>
      <pre className={`${styles.stream_text} ${error ? styles.stream_text_err : ""}`}>
        {truncated ? text.slice(0, MAX_STREAM_CHARS) : text}
      </pre>
      {truncated && (
        <span className={styles.more}>
          … {(text.length - MAX_STREAM_CHARS).toLocaleString()} more chars
        </span>
      )}
    </div>
  );
}

function BashBody({ block, meta }: { block: ToolUseBlockType; meta: unknown }) {
  const bash = asBashResult(meta);
  if (!bash) return null;
  const command = typeof block.input.command === "string" ? block.input.command : "";
  const noOutput = bash.stdout.trim() === "" && bash.stderr.trim() === "";
  return (
    <>
      {command && <pre className={styles.command}>{command}</pre>}
      {bash.stdout.trim() !== "" && <Stream label="stdout" text={bash.stdout} />}
      {bash.stderr.trim() !== "" && <Stream label="stderr" text={bash.stderr} error />}
      {noOutput && (
        <div className={styles.note}>
          {bash.noOutputExpected ? "No output expected." : "No output."}
        </div>
      )}
    </>
  );
}

function AgentBody({ meta }: { meta: unknown }) {
  const agent = asAgentResult(meta);
  const [showPrompt, setShowPrompt] = useState(false);
  if (!agent) return null;
  const stats = Object.entries(agent.toolStats ?? {}).filter(([, v]) => v > 0);
  return (
    <>
      <div className={styles.agent_meta}>
        {agent.resolvedModel && <span className={styles.chip}>{agent.resolvedModel}</span>}
        <span className={styles.chip}>{agent.status}</span>
        {agent.totalToolUseCount !== undefined && (
          <span className={styles.chip}>{agent.totalToolUseCount} tool calls</span>
        )}
      </div>
      {stats.length > 0 && (
        <div className={styles.agent_stats}>
          {stats.map(([k, v]) => (
            <span key={k} className={styles.stat}>
              <span className={styles.stat_key}>{k.replace(/Count$/, "")}</span>
              <span className={styles.stat_val}>{v}</span>
            </span>
          ))}
        </div>
      )}
      {agent.prompt && (
        <>
          <button className={styles.toggle} onClick={() => setShowPrompt((v) => !v)}>
            {showPrompt ? "▾" : "▸"} prompt
          </button>
          {showPrompt && <pre className={styles.stream_text}>{agent.prompt}</pre>}
        </>
      )}
      {agent.content && <Stream label="result" text={agent.content} />}
    </>
  );
}

const TODO_MARK: Record<string, string> = {
  completed: "☑",
  in_progress: "▶",
  pending: "☐",
};

function TodoBody({ meta }: { meta: unknown }) {
  const todo = asTodoWriteResult(meta);
  if (!todo) return null;
  // Items absent from the previous list are new in this write; marking them
  // makes a long re-stated plan readable at a glance.
  const before = new Set(todo.oldTodos.map((it) => it.content));
  return (
    <ul className={styles.todo_list}>
      {todo.newTodos.map((item, i) => (
        <li
          key={i}
          className={`${styles.todo_item} ${item.status === "completed" ? styles.todo_done : ""} ${
            item.status === "in_progress" ? styles.todo_current : ""
          }`}
        >
          <span className={styles.todo_mark} aria-hidden>
            {TODO_MARK[item.status] ?? "☐"}
          </span>
          <span className={styles.todo_text}>{item.content}</span>
          {!before.has(item.content) && todo.oldTodos.length > 0 && (
            <span className={styles.todo_new}>new</span>
          )}
        </li>
      ))}
    </ul>
  );
}

/**
 * The custom body for a tool, or null to fall back to the generic
 * input-JSON-plus-result rendering.
 *
 * Edit/Write are absent here on purpose: their body is a diff, which the caller
 * already owns via `DiffView` (it now feeds on `structuredPatch`).
 */
export function ToolBody({
  block,
  meta,
  result,
}: {
  block: ToolUseBlockType;
  meta: unknown;
  result?: ToolResultBlock;
}): ReactNode {
  if (meta === undefined || result?.is_error) return null;
  if (block.name === "Bash") return <BashBody block={block} meta={meta} />;
  if (block.name === "Agent") return <AgentBody meta={meta} />;
  if (block.name === "TodoWrite") return <TodoBody meta={meta} />;
  return null;
}

// ── Read-only group label ────────────────────────────────────────────────────

/**
 * Label for a run of grouped read-only calls. The old label hard-coded
 * "Explored N files" even when the group was WebSearch or TodoWrite.
 */
export function groupLabel(names: string[]): string {
  const unique = [...new Set(names)];
  const n = names.length;
  if (unique.length === 1) {
    const only = unique[0];
    if (only === "Read") return `Read ${n} file${n === 1 ? "" : "s"}`;
    if (only === "Grep" || only === "Glob") return `Searched ${n} time${n === 1 ? "" : "s"}`;
    if (only === "WebSearch") return `Searched the web ${n} time${n === 1 ? "" : "s"}`;
    if (only === "WebFetch") return `Fetched ${n} page${n === 1 ? "" : "s"}`;
    if (only === "TodoWrite" || only === "TodoRead") return `Updated the plan`;
    return `${only} ×${n}`;
  }
  return `Explored (${n} calls)`;
}
