// 详情页 header 右侧的图标动作按钮，一处。
//
// `AppHeader` 把 header 的骨架收成了一个组件，但它的 `actions` 槽是个
// `ReactNode`——于是每个需要「刷新」的页各自搓了一份。改前逐个数过四份：
//
//   页            字形                     颜色                   padding
//   知识库        <RefreshCw size={16}/>   --color-text-secondary  2px 6px（外加一条
//                                                                 遗留的 margin-left:auto）
//   仓库          "⟳" 文本字符             --color-text-secondary  2px 6px
//   计划          "⟳" 文本字符             --color-text-dim        4px 6px
//   账号与用量    "⟳" 文本字符             --color-text-secondary  2px 6px（唯一一个
//                                                                 写了 :disabled）
//
// 那个 `⟳`（U+27F3）是最要紧的一处：它是**文本字符**，字形随系统字体走，跟同
// 一个 header 里左边那个 lucide 描边 chevron 不是一套笔画，粗细和大小都对不上。
// 三个页用它、一个页用图标，正是「像网页而不像 app」那种观感最具体的来源之一。
//
// 顺带补上四份里一份都没有的东西：命中区（32px 的可见方框 + 44px 的隐形命中
// 区，与 .backButton 镜像）和 busy 时的旋转——按下去几百毫秒的网络往返里没有
// 任何反馈，在手机上就等于「这个按钮坏了」。
//
// 刻意不做成 `AppHeader` 的一个 `onRefresh` prop：动作不止刷新一种（知识库文档
// 页还有导出、版本选择），把它们逐个变成 prop 会重演 AppHeader 顶部注释里说的
// 那种「每加一个逃生口就重新打开一次漂移」。这里给的是**一颗按钮**的共享实现，
// 而不是一个动作清单的抽象。

import type { ReactNode } from "react";
import styles from "./HeaderAction.module.css";

export function HeaderAction({
  icon,
  label,
  onClick,
  busy,
  disabled,
}: {
  /** 一个 lucide 图标节点。用图标而不是文本字符：文本 glyph 的笔画随系统字体
   *  变，跟 header 左边那个描边 chevron 配不上。 */
  icon: ReactNode;
  /** 无障碍名称（按钮上没有可见文字）。 */
  label: string;
  onClick: () => void;
  /** 正在跑 —— 图标转起来。不自动置灰：转动本身已经说了「在忙」，再置灰会让
   *  「在忙」和「不可用」两件事共用一种长相。 */
  busy?: boolean;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      className={styles.action}
      onClick={onClick}
      aria-label={label}
      aria-busy={busy || undefined}
      data-busy={busy ? "true" : undefined}
      disabled={disabled}
    >
      {icon}
    </button>
  );
}
