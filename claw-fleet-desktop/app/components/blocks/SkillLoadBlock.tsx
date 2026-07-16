import { useState } from "react";
import { useTranslation } from "react-i18next";
import { TextBlock } from "./TextBlock";
import styles from "./SkillLoadBlock.module.css";

interface Props {
  slug: string;
  body: string;
}

function formatSize(n: number): string {
  if (n < 1000) return `${n} chars`;
  if (n < 1_000_000) return `${(n / 1000).toFixed(1)}k chars`;
  return `${(n / 1_000_000).toFixed(2)}M chars`;
}

/**
 * The SKILL.md body Claude Code injects when a skill loads, folded into a
 * collapsed divider row — the same visual language as the context-compaction
 * banner (`CompactSummaryBlock`), because both are harness injections rather
 * than turns the user took. Expands to the full markdown on click.
 */
export function SkillLoadBlock({ slug, body }: Props) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  return (
    <div className={styles.root}>
      <button className={styles.header} onClick={() => setOpen((o) => !o)}>
        <span className={styles.line} />
        <span className={styles.label}>
          <span className={styles.icon}>⚙</span>
          {t("detail.skill_loaded", "已加载 SKILL")}
          <span className={styles.slug}>{slug}</span>
          <span className={styles.size}>· {formatSize(body.length)}</span>
          <span className={styles.arrow}>{open ? "▾" : "▸"}</span>
        </span>
        <span className={styles.line} />
      </button>
      {open && (
        <div className={styles.body}>
          <TextBlock text={body} />
        </div>
      )}
    </div>
  );
}
