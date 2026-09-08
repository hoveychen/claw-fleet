import type { ReactNode } from "react";
import { ChevronLeft } from "lucide-react";
import { t } from "../i18n";
import styles from "./AppHeader.module.css";

/** The detail-page header, in one place.
 *
 * Nine views (产出详情 / 知识库列表 / 知识库文档 / 仓库 / 仓库详情 / 计划 /
 * 用量 / 终端 / 会话详情) each hand-copied the same `.header` + `.backButton`
 * CSS block and the same JSX shape. Eight of the nine stayed in sync by luck;
 * the ninth drifted to a different padding and title size, which is what made
 * 会话详情 read as belonging to a different app. Nothing enforced the shared
 * shape because there was no shared shape — only a convention re-typed nine
 * times.
 *
 * Every prop below exists because some page needs it today. There is
 * deliberately no `as`, no size variant and no colour override: all nine want
 * the same chrome, and the drift this component exists to end is exactly what
 * such escape hatches re-enable. */
export function AppHeader({
  onBack,
  title,
  titleAfter,
  sub,
  actions,
  seamless,
}: {
  onBack: () => void;
  /** A plain string gets the standard title treatment. 会话详情 passes a node
   *  because its title row also carries a subagent badge and is itself a tap
   *  target that unfolds the info panel. */
  title: ReactNode;
  /** A small badge riding immediately after a string title — 知识库's entry
   *  count. Only meaningful with a string title; a node title lays out its own
   *  row and puts whatever it needs in there. */
  titleAfter?: ReactNode;
  /** Second line, dim and small — 仓库详情's repo path, 知识库文档's slug. */
  sub?: ReactNode;
  /** Trailing controls: refresh, export, version select, the ☰ menu. */
  actions?: ReactNode;
  /** Drop the bottom hairline. For a header with a tab strip directly under it,
   *  where the line would split one panel into two slabs. */
  seamless?: boolean;
}) {
  return (
    <header className={styles.header} data-seamless={seamless ? "true" : undefined}>
      <button className={styles.backButton} onClick={onBack} aria-label={t("返回")}>
        <ChevronLeft size={20} />
      </button>
      <div className={styles.text}>
        {typeof title === "string" ? (
          <div className={styles.titleRow}>
            <div className={styles.title}>{title}</div>
            {titleAfter}
          </div>
        ) : (
          title
        )}
        {sub !== undefined && <div className={styles.sub}>{sub}</div>}
      </div>
      {actions !== undefined && <div className={styles.actions}>{actions}</div>}
    </header>
  );
}
