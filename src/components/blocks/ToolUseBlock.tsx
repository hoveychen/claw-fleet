import { useState } from "react";
import type { ToolResultBlock, ToolUseBlock as ToolUseBlockType } from "../../types";
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

export function ToolUseBlock({ block, result, isPartial }: Props) {
  const [open, setOpen] = useState(false);
  const summary = formatInput(block.input);
  const isReadOnly = READ_ONLY_TOOLS.has(block.name);

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
          <div className={styles.input_section}>
            <span className={styles.section_label}>Input</span>
            <pre className={styles.input_text}>
              {JSON.stringify(block.input, null, 2)}
            </pre>
          </div>
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
