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
import { ToolBody, groupLabel, hasCustomBody, headerStats } from "./toolPresenters";
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
  // Show a compact one-liner for common tools
  if ("command" in input) return String(input.command);
  if ("file_path" in input) return relToWorkspace(String(input.file_path), wsRoot);
  if ("pattern" in input) return String(input.pattern);
  if ("path" in input) return relToWorkspace(String(input.path), wsRoot);
  if ("query" in input) return String(input.query);
  if ("url" in input) return String(input.url);
  return JSON.stringify(input, null, 2);
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

  const summary = formatInput(block.input, block.name, paths?.workspaceRoot);
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
            <ToolBody block={block} meta={meta} result={result} />
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
