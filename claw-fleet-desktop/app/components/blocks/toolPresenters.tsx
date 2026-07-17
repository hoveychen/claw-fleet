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
import { useTranslation } from "react-i18next";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useAgentNav } from "../AgentNavContext";
import {
  asAgentResult,
  asBashResult,
  asErrorMessage,
  asFileEditResult,
  asFileSearchResult,
  asTodoWriteResult,
  asWebSearchResult,
  bashExitCode,
  diffStats,
} from "../../toolResults";
import type { ToolResultBlock, ToolUseBlock as ToolUseBlockType } from "../../types";
import type { PathLinkContext } from "../../markdown/pathLinks";
import { ExpandableText } from "./ExpandableText";
import { TextBlock } from "./TextBlock";
import styles from "./toolPresenters.module.css";

/**
 * Tools whose body this module renders itself. Edit/Write are absent by design:
 * their body is a diff, owned by the caller's `DiffView`.
 */
const CUSTOM_BODY = new Set(["Bash", "Agent", "TodoWrite", "WebSearch"]);

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

  if (block.name === "WebSearch") {
    const search = asWebSearchResult(meta);
    if (!search) return null;
    const n = search.links.length;
    return <span className={styles.chip}>{n} result{n === 1 ? "" : "s"}</span>;
  }

  return null;
}

// ── Bodies ───────────────────────────────────────────────────────────────────

/** A named run of tool output — stdout, stderr, a subagent's answer. */
function Stream({ label, text, error }: { label: string; text: string; error?: boolean }) {
  return (
    <div className={styles.stream}>
      <span className={`${styles.stream_label} ${error ? styles.stream_label_err : ""}`}>
        {label}
      </span>
      <ExpandableText text={text} className={error ? styles.stream_text_err : ""} />
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
  const { t } = useTranslation();
  const agent = asAgentResult(meta);
  const nav = useAgentNav();
  const [showPrompt, setShowPrompt] = useState(false);
  if (!agent) return null;
  const stats = Object.entries(agent.toolStats ?? {}).filter(([, v]) => v > 0);
  const agentId = agent.agentId;
  const canOpen = agentId !== undefined && nav !== null && nav.has(agentId);
  return (
    <>
      <div className={styles.agent_meta}>
        {agent.resolvedModel && <span className={styles.chip}>{agent.resolvedModel}</span>}
        <span className={styles.chip}>{agent.status}</span>
        {agent.totalToolUseCount !== undefined && (
          <span className={styles.chip}>{agent.totalToolUseCount} tool calls</span>
        )}
        {/* The card only summarises. The subagent's own transcript is where you
            can see what it actually did. */}
        {canOpen && agentId && (
          <button
            type="button"
            className={styles.open_agent}
            onClick={() => nav.open(agentId)}
          >
            {t("detail.open_subagent")} →
          </button>
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
          {showPrompt && (
            <div className={styles.agent_prompt_body}>
              <TextBlock text={agent.prompt} />
            </div>
          )}
        </>
      )}
      {/* The subagent's prompt (above) and final result are both model/agent-
          authored prose that routinely carries markdown — render them through
          TextBlock like AgentInput and TaskNotification do, not as raw <pre>. */}
      {agent.content && (
        <div className={styles.agent_prompt}>
          <span className={styles.stream_label}>result</span>
          <div className={styles.agent_prompt_body}>
            <TextBlock text={agent.content} />
          </div>
        </div>
      )}
    </>
  );
}

/**
 * The Agent card's *input*, rendered readably instead of as raw JSON.
 *
 * A subagent call carries a short `description` (the 3-5 word task name) and a
 * long `prompt` (the full brief). Before this, whenever no `toolUseResult` was
 * recorded — i.e. while the subagent is still running, or for non-Claude
 * sources — the card fell through to the generic `JSON.stringify(input)` branch,
 * so the prompt showed as one blob with literal `\n` escapes. Rendering the
 * prompt as Markdown — via the transcript's own `TextBlock` — turns the
 * `## headings`, fenced diffs and `code` spans into real structure, matching
 * how assistant text renders elsewhere. A bounded, scrollable box keeps a long
 * brief from swamping the conversation.
 */
export function AgentInput({
  block,
  paths,
}: {
  block: ToolUseBlockType;
  paths?: PathLinkContext;
}) {
  const desc = typeof block.input.description === "string" ? block.input.description.trim() : "";
  const prompt = typeof block.input.prompt === "string" ? block.input.prompt : "";
  const subagentType =
    typeof block.input.subagent_type === "string" ? block.input.subagent_type.trim() : "";
  return (
    <>
      {(desc || subagentType) && (
        <div className={styles.agent_meta}>
          {subagentType && <span className={styles.chip}>{subagentType}</span>}
          {desc && <span className={styles.agent_task}>{desc}</span>}
        </div>
      )}
      {prompt && (
        <div className={styles.agent_prompt}>
          <span className={styles.stream_label}>prompt</span>
          <div className={styles.agent_prompt_body}>
            <TextBlock text={prompt} paths={paths} />
          </div>
        </div>
      )}
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

/** Favicon for a search-result row, via Google's favicon service. Falls back
 *  to a neutral tile when offline / blocked — the row must never lose its
 *  gutter alignment just because an icon failed to load. */
function Favicon({ url }: { url: string }) {
  const [failed, setFailed] = useState(false);
  let host = "";
  try {
    host = new URL(url).hostname;
  } catch {
    // Malformed URL — keep the fallback tile.
  }
  if (!host || failed) return <span className={styles.favicon_fallback} aria-hidden />;
  return (
    <img
      className={styles.favicon}
      src={`https://www.google.com/s2/favicons?domain=${encodeURIComponent(host)}&sz=32`}
      alt=""
      loading="lazy"
      onError={() => setFailed(true)}
    />
  );
}

/** `WebSearch`: the query as a headline over one row per returned link —
 *  favicon, title, domain — the claude.ai search-card look. Narration strings
 *  in the payload are dropped; the links are the substance. */
function WebSearchBody({ block, meta, rail }: { block: ToolUseBlockType; meta: unknown; rail?: boolean }) {
  const search = asWebSearchResult(meta);
  if (!search) return null;
  // In rail mode the collapsed summary line (the query) stays visible above
  // the expanded body, so repeating it here would read twice in a row.
  const query = rail
    ? ""
    : search.query || (typeof block.input.query === "string" ? block.input.query : "");
  return (
    <>
      {query && <div className={styles.search_query}>{query}</div>}
      {search.links.length > 0 ? (
        <ul className={styles.search_results}>
          {search.links.map((l, i) => {
            let host = "";
            try {
              host = new URL(l.url).hostname.replace(/^www\./, "");
            } catch {
              // Row still renders; it just has no domain tag.
            }
            return (
              <li key={i}>
                <a
                  className={styles.search_result}
                  href={l.url}
                  onClick={(e) => {
                    e.preventDefault();
                    // Same surfaced-error pattern as SafeLink: a missing
                    // opener capability must not fail silently.
                    openUrl(l.url).catch((err) => console.error("openUrl failed:", l.url, err));
                  }}
                >
                  <Favicon url={l.url} />
                  <span className={styles.search_title}>{l.title}</span>
                  {host && <span className={styles.search_domain}>{host}</span>}
                </a>
              </li>
            );
          })}
        </ul>
      ) : (
        <div className={styles.note}>No results.</div>
      )}
    </>
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
  rail,
}: {
  block: ToolUseBlockType;
  meta: unknown;
  result?: ToolResultBlock;
  rail?: boolean;
}): ReactNode {
  if (meta === undefined || result?.is_error) return null;
  if (block.name === "Bash") return <BashBody block={block} meta={meta} />;
  if (block.name === "Agent") return <AgentBody meta={meta} />;
  if (block.name === "TodoWrite") return <TodoBody meta={meta} />;
  if (block.name === "WebSearch") return <WebSearchBody block={block} meta={meta} rail={rail} />;
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
