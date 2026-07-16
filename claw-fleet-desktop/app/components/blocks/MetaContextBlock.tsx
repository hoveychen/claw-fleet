import { useState } from "react";
import { useTranslation } from "react-i18next";
import { TextBlock } from "./TextBlock";
// Reuse the SkillLoadBlock divider styling verbatim — both are harness/runtime
// injections folded into the same collapsed-divider visual language.
import styles from "./SkillLoadBlock.module.css";

interface Props {
  body: string;
}

function formatSize(n: number): string {
  if (n < 1000) return `${n} chars`;
  if (n < 1_000_000) return `${(n / 1000).toFixed(1)}k chars`;
  return `${(n / 1_000_000).toFixed(2)}M chars`;
}

/**
 * Codex injects boilerplate system context — its sandbox/permissions preamble,
 * the `/root` multi-agent collaboration prompt, `<multi_agent_mode>` guidance —
 * as `role:"developer"` messages (tagged `isMeta` in codex_source.rs). None of
 * it is user-authored, so fold it into a collapsed divider row, the same visual
 * language as `SkillLoadBlock` / `CompactSummaryBlock`. Expands on click.
 */
export function MetaContextBlock({ body }: Props) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const label = deriveMetaLabel(body, t);
  return (
    <div className={styles.root}>
      <button className={styles.header} onClick={() => setOpen((o) => !o)}>
        <span className={styles.line} />
        <span className={styles.label}>
          <span className={styles.icon}>⚙</span>
          {t("detail.codex_meta", "系统上下文")}
          <span className={styles.slug}>{label}</span>
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

/**
 * A short human tag for the fold header so the reader can tell which codex
 * injection it is without expanding. Matches the three developer-role blocks
 * codex emits; falls back to a generic label for anything else.
 */
function deriveMetaLabel(body: string, t: (k: string, d: string) => string): string {
  const head = body.slice(0, 200);
  if (head.startsWith("<permissions instructions>"))
    return t("detail.codex_meta_permissions", "权限 / 沙箱");
  if (head.includes("primary agent in a team"))
    return t("detail.codex_meta_collab", "多智能体协作");
  if (head.startsWith("<multi_agent_mode>"))
    return t("detail.codex_meta_multiagent", "multi_agent_mode");
  return t("detail.codex_meta_generic", "注入指令");
}
