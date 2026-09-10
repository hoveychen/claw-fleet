import { useState } from "react";
import ReactMarkdown from "react-markdown";
import { Check } from "lucide-react";
import { mdRemarkPlugins, mdRehypePlugins } from "../markdown/plugins";
import { mdComponents, mdInlineComponents } from "../markdown/components";
import { splitMarker, taskTip } from "../../../shared-ts/taskItem";
import styles from "./TaskItemLine.module.css";

/** 一条 P 在计划里的位置。`current` 是第一个待办项（会话页用桌面下发的
 *  currentTask 比对，计划页取第一个未完成的）。 */
export type TaskItemState = "done" | "current" | "pending";

/**
 * 单条 P 任务。会话详情的任务页签与计划页共用一份 —— 两处此前各写了一份
 * 近乎相同的实现，且都只把 `**` 剥掉而不渲染。
 *
 * 两条约束定了它的形状。一是 P-task 正文常常是几百字的实现笔记，所以默认
 * 压成一行，点一下才展开：整段铺开会把页签变成一堵墙。二是正文是 markdown，
 * 而压行态和展开态要的是两种渲染：压行态用 inline 组件表（`p` 展平成
 * fragment，整项塌成一行），展开态用块级表，多段落笔记保住段落分隔。
 *
 * 与桌面的 `TaskLine` 是同一份契约（marker 抽成徽章 + 正文走真 markdown），
 * 拆 marker 的那一份逻辑本身也共用 `shared-ts/taskItem`。
 */
export function TaskItemLine({
  text,
  state,
  startOpen,
}: {
  text: string;
  state: TaskItemState;
  /** 挂载即展开 —— 计划页从矩阵某一格点进来时用。 */
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
