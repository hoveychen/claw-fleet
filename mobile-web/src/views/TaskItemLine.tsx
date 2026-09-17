import { useState } from "react";
import ReactMarkdown from "react-markdown";
import { Check } from "lucide-react";
import { mdRemarkPlugins, mdRehypePlugins } from "../markdown/plugins";
import { mdComponents, mdInlineComponents } from "../markdown/components";
import { splitMarker, taskTip } from "../../../shared-ts/taskItem";
import styles from "./TaskItemLine.module.css";

/** Position of one P in the plan. `current` is the first pending item (session page compares with
 *  desktop-provided currentTask, plan page takes the first uncompleted). */
export type TaskItemState = "done" | "current" | "pending";

/**
 * Single P task. Reused between session detail task tab and plan page — both previously wrote
 * nearly identical implementations, both only strip `**` without rendering.
 *
 * Two constraints shape it. First, P-task body is often hundreds of words of implementation notes,
 * so collapsed to one line by default, expands on click: laying it all out turns the tab into a wall.
 * Second, body is markdown, and collapsed vs. expanded need two renderings: collapsed uses inline
 * components (`p` flattened to fragment, whole item collapses to one line), expanded uses block-level,
 * preserving paragraph breaks in multi-paragraph notes.
 *
 * Same contract as desktop's `TaskLine` (marker extracted as badge + body rendered as true markdown),
 * the logic for splitting marker is also shared from `shared-ts/taskItem`.
 */
export function TaskItemLine({
  text,
  state,
  startOpen,
}: {
  text: string;
  state: TaskItemState;
  /** Expanded on mount — used when plan page enters from a grid cell. */
  startOpen?: boolean;
}) {
  const { marker, rest } = splitMarker(text);
  const [open, setOpen] = useState(!!startOpen);
  return (
    <div
      className={styles.row}
      data-state={state}
      data-open={open || undefined}
      role="button"
      tabIndex={0}
      aria-expanded={open}
      title={open ? undefined : taskTip(text)}
      onClick={() => setOpen((v) => !v)}
    >
      <span className={styles.box} aria-hidden>
        {state === "done" ? <Check size={11} /> : null}
      </span>
      {marker && <span className={styles.marker}>{marker}</span>}
      <span className={styles.text}>
        <ReactMarkdown
          remarkPlugins={mdRemarkPlugins}
          rehypePlugins={mdRehypePlugins}
          components={open ? mdComponents : mdInlineComponents}
        >
          {rest}
        </ReactMarkdown>
      </span>
    </div>
  );
}
