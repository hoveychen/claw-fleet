import { useState } from "react";
import ReactMarkdown from "react-markdown";
import { markdownUrlTransform } from "../markdown/plugins";
import {
  inlineMarkdownComponents,
  safeMarkdownComponents,
  safeRemarkPlugins,
  safeRehypePlugins,
} from "../markdown/safeLinks";
import { splitMarker, taskTip } from "../../../shared-ts/taskItem";
import styles from "./TaskLine.module.css";

// `splitMarker` / `taskTip` live in shared-ts/ so the phone renders a P-task
// the same way; re-exported here because every desktop call site imports them
// alongside the component.
export { splitMarker, taskTip };

/** Where a P-task stands in its plan. `current` is the first still-pending
 *  item — the visible answer to 「做到第几个 P 了」. */
export type TaskLineState = "done" | "current" | "pending";

/**
 * One P-task row, shared by the 计划树 drawer and the session detail's 任务
 * facet — the two places a plan's items are read.
 *
 * Two constraints shape it. First, items routinely run to several paragraphs
 * of implementation notes, so the row is clamped to two lines until clicked;
 * rendering them in full is what turned both surfaces into a wall of text.
 * Second, the text is markdown, and the clamped and open states want different
 * renderings of it: clamped uses the inline component set (`p` unwrapped) so
 * the whole item collapses onto one visual line, while open uses the block set
 * so a multi-paragraph note keeps its paragraph breaks.
 *
 * Deliberately not a `<button>`: an item's markdown may contain a link, and a
 * link nested in a button is invalid HTML that browsers resolve unpredictably.
 */
export function TaskLine({
  text,
  state,
  startOpen,
}: {
  text: string;
  state: TaskLineState;
  /** Open on mount — the 计划树 uses it when a click landed on this cell. */
  startOpen?: boolean;
}) {
  const { marker, rest } = splitMarker(text);
  const [open, setOpen] = useState(!!startOpen);
  const toggle = () => setOpen((v) => !v);
  return (
    <div
      className={styles.row}
      data-state={state}
      data-open={open || undefined}
      role="button"
      tabIndex={0}
      aria-expanded={open}
      title={open ? undefined : taskTip(text)}
      onClick={toggle}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          toggle();
        }
      }}
    >
      <span className={styles.box} aria-hidden>
        {state === "done" ? "☑" : state === "current" ? "▸" : "☐"}
      </span>
      {marker && <span className={styles.marker}>{marker}</span>}
      <span className={styles.text}>
        <ReactMarkdown
          urlTransform={markdownUrlTransform}
          remarkPlugins={safeRemarkPlugins}
          rehypePlugins={safeRehypePlugins}
          components={open ? safeMarkdownComponents : inlineMarkdownComponents}
        >
          {rest}
        </ReactMarkdown>
      </span>
    </div>
  );
}
