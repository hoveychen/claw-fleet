// 会话详情页头部下面那条「活状态轨」。
//
// 它顶掉的是一条六个 tab 的条（消息/决策/计划/Token/Workflow/接力）。那条 tab
// 在 390px 宽的屏上每个标签只剩约 46px，且六个标签一个数字都不带——你得逐个
// 点进去才知道哪个有东西。这条轨反过来：它不给你六个等权重的入口，它只说这个
// 会话此刻在发生什么，而每一句话顺便就是那件事的入口。
//
// 空的时候整条不渲染（不是渲染成一条空白带）——安静的会话不该为一行视觉噪音
// 付出 34px。内容规则见 sessionStatusPills.ts。

import { t } from "../i18n";
import type { PillTarget, StatusPill } from "./sessionStatusPills";
import styles from "./StatusRail.module.css";

export function StatusRail({
  pills,
  onOpen,
}: {
  pills: StatusPill[];
  onOpen: (target: PillTarget) => void;
}) {
  if (pills.length === 0) return null;
  return (
    // role=list 而不是 nav：这些首先是读数，其中一部分恰好可点。用 nav 会让
    // 读屏软件把「上下文 40%」念成一个导航目标。
    <div className={styles.rail} role="list" aria-label={t("会话状态")}>
      {pills.map((p) =>
        p.target ? (
          <button
            key={p.key}
            type="button"
            role="listitem"
            className={styles.pill}
            data-tone={p.tone}
            onClick={() => onOpen(p.target as PillTarget)}
          >
            {p.dot && <i className={styles.dot} aria-hidden="true" />}
            {p.label}
          </button>
        ) : (
          <span key={p.key} role="listitem" className={styles.pill} data-tone={p.tone}>
            {p.dot && <i className={styles.dot} aria-hidden="true" />}
            {p.label}
          </span>
        ),
      )}
    </div>
  );
}
