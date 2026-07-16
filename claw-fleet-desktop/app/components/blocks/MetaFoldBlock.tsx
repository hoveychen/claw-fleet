import { useState } from "react";
import { useTranslation } from "react-i18next";
import { TextBlock } from "./TextBlock";
import { parseSkillInjection } from "../../skillInjection";
import styles from "./MetaFoldBlock.module.css";

interface Props {
  body: string;
}

function formatSize(n: number): string {
  if (n < 1000) return `${n} chars`;
  if (n < 1_000_000) return `${(n / 1000).toFixed(1)}k chars`;
  return `${(n / 1_000_000).toFixed(2)}M chars`;
}

/**
 * The single collapsed-divider fold for every synthetic `isMeta` user turn —
 * content the harness/runtime feeds the agent rather than something the user
 * typed. Covers both the SKILL.md body a `Skill` load injects and codex's
 * developer-role boilerplate (sandbox/permissions preamble, the `/root`
 * multi-agent collaboration prompt, `<multi_agent_mode>` guidance). The header
 * self-labels from the body, so a skill load still reads as a skill while
 * everything else reads as generic system context. Same visual language as
 * `CompactSummaryBlock`. Expands to the full markdown on click.
 */
export function MetaFoldBlock({ body }: Props) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const { title, tag } = deriveHeader(body, t);
  return (
    <div className={styles.root}>
      <button className={styles.header} onClick={() => setOpen((o) => !o)}>
        <span className={styles.line} />
        <span className={styles.label}>
          <span className={styles.icon}>⚙</span>
          {title}
          <span className={styles.slug}>{tag}</span>
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
 * Self-label the fold from its body: a SKILL.md injection reads as the loaded
 * skill (slug tag); codex's three known developer-role blocks get a short human
 * tag; anything else falls back to a generic "injected instructions" label.
 */
function deriveHeader(
  body: string,
  t: (k: string, d: string) => string,
): { title: string; tag: string } {
  const skill = parseSkillInjection(body);
  if (skill) {
    return { title: t("detail.skill_loaded", "已加载 SKILL"), tag: skill.slug };
  }
  const head = body.slice(0, 200);
  const title = t("detail.codex_meta", "系统上下文");
  if (head.startsWith("<permissions instructions>"))
    return { title, tag: t("detail.codex_meta_permissions", "权限 / 沙箱") };
  if (head.includes("primary agent in a team"))
    return { title, tag: t("detail.codex_meta_collab", "多智能体协作") };
  if (head.startsWith("<multi_agent_mode>"))
    return { title, tag: t("detail.codex_meta_multiagent", "multi_agent_mode") };
  return { title, tag: t("detail.codex_meta_generic", "注入指令") };
}
