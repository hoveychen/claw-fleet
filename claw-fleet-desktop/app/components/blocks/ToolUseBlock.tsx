import { useState } from "react";
import type { ToolResultBlock, ToolUseBlock as ToolUseBlockType } from "../../types";
import { DiffView } from "./DiffView";
import styles from "./ToolUseBlock.module.css";

// Read-only tools that get grouped into "Explored [N]"
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
  /**
   * For Write: file content immediately before this tool ran (reconstructed
   * by replaying prior Read/Edit/Write ops in the same session). `null` means
   * the prior content is unknown — render as a "new file" diff.
   */
  baseline?: string | null;
}

interface MultiEditEdit {
  old_string: string;
  new_string: string;
  replace_all?: boolean;
}

function formatInput(input: Record<string, unknown>): string {
  // Show a compact one-liner for common tools
  if ("command" in input) return String(input.command);
  if ("file_path" in input) return String(input.file_path);
  if ("pattern" in input) return String(input.pattern);
  if ("path" in input) return String(input.path);
  if ("query" in input) return String(input.query);
  if ("url" in input) return String(input.url);
  return JSON.stringify(input, null, 2);
}

function ResultContent({ result }: { result: ToolResultBlock }) {
  const content =
    typeof result.content === "string"
      ? result.content
      : JSON.stringify(result.content, null, 2);

  // Truncate very long results
  const MAX = 2000;
  const truncated = content.length > MAX;
  const display = truncated ? content.slice(0, MAX) : content;

  return (
    <div className={`${styles.result} ${result.is_error ? styles.result_error : ""}`}>
      <pre className={styles.result_text}>{display}</pre>
      {truncated && (
        <span className={styles.truncated}>
          … {(content.length - MAX).toLocaleString()} more chars
        </span>
      )}
    </div>
  );
}

/** Render Edit/MultiEdit/Write input as a diff view; falls back to null. */
function DiffSection({ block, baseline }: { block: ToolUseBlockType; baseline?: string | null }) {
  const input = block.input;
  const filePath = typeof input.file_path === "string" ? input.file_path : undefined;

  if (block.name === "Edit") {
    const oldS = typeof input.old_string === "string" ? input.old_string : "";
    const newS = typeof input.new_string === "string" ? input.new_string : "";
    return <DiffView filePath={filePath} before={oldS} after={newS} tag="Edit" />;
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
    // baseline === undefined or null means no replay info available; the diff
    // view will render content as all-additions ("new file" style).
    const before = baseline === undefined ? null : baseline;
    return <DiffView filePath={filePath} before={before} after={content} tag="Write" />;
  }

  return null;
}

const DIFF_TOOLS = new Set(["Edit", "MultiEdit", "Write"]);

export function ToolUseBlock({ block, result, isPartial, baseline }: Props) {
  const [open, setOpen] = useState(false);
  const summary = formatInput(block.input);
  const isReadOnly = READ_ONLY_TOOLS.has(block.name);
  const isDiffTool = DIFF_TOOLS.has(block.name);

  return (
    <div className={`${styles.root} ${isReadOnly ? styles.readonly : ""}`}>
      <button className={styles.header} onClick={() => setOpen((o) => !o)}>
        <span className={styles.arrow}>{open ? "▾" : "▸"}</span>
        <span className={styles.tool_name}>{block.name}</span>
        {!open && (
          <span className={styles.summary}>{summary}</span>
        )}
        {isPartial && !result && (
          <span className={styles.spinner}>⟳</span>
        )}
        {result?.is_error && !open && (
          <span className={styles.error_badge}>error</span>
        )}
        {result && !result.is_error && !open && (
          <span className={styles.ok_dot} />
        )}
      </button>

      {open && (
        <div className={styles.body}>
          {isDiffTool ? (
            <div className={styles.input_section}>
              <DiffSection block={block} baseline={baseline} />
            </div>
          ) : (
            <div className={styles.input_section}>
              <span className={styles.section_label}>Input</span>
              <pre className={styles.input_text}>
                {JSON.stringify(block.input, null, 2)}
              </pre>
            </div>
          )}
          {result && <ResultContent result={result} />}
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
  blocks: Array<{ block: ToolUseBlockType; result?: ToolResultBlock }>;
}

export function GroupedToolUseBlocks({ blocks }: GroupedProps) {
  const [open, setOpen] = useState(false);

  return (
    <div className={styles.group}>
      <button className={styles.group_toggle} onClick={() => setOpen((o) => !o)}>
        <span className={styles.arrow}>{open ? "▾" : "▸"}</span>
        <span className={styles.group_label}>
          Explored {blocks.length} file{blocks.length !== 1 ? "s" : ""}
        </span>
      </button>
      {open && (
        <div className={styles.group_body}>
          {blocks.map(({ block, result }, i) => (
            <ToolUseBlock key={block.id ?? i} block={block} result={result} />
          ))}
        </div>
      )}
    </div>
  );
}
