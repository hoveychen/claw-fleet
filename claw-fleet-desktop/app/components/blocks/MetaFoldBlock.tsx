import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { TextBlock } from "./TextBlock";
import { parseSkillInjection } from "../../skillInjection";
import styles from "./MetaFoldBlock.module.css";

interface Props {
  /**
   * One entry per synthetic `isMeta` user turn folded into this card. A single
   * turn passes a one-element array; a run of adjacent turns passes them all so
   * they collapse into one divider instead of stacking N identical rows.
   */
  segments: string[];
  /** True while the active search hit lives inside this fold. The fold follows
   *  the search: it opens when the reader steps onto the hit and closes again
   *  when they step off (manual toggles still work in between). */
  forceOpen?: boolean;
}

function formatSize(n: number): string {
  if (n < 1000) return `${n} chars`;
  if (n < 1_000_000) return `${(n / 1000).toFixed(1)}k chars`;
  return `${(n / 1_000_000).toFixed(2)}M chars`;
}

/**
 * The collapsed-divider fold for synthetic `isMeta` user turns — content the
 * harness/runtime feeds the agent rather than something the user typed. Covers
 * both the SKILL.md body a `Skill` load injects and codex's developer-role
 * boilerplate (sandbox/permissions preamble, the `/root` multi-agent
 * collaboration prompt, `<multi_agent_mode>` guidance). A single turn self-labels
 * from its body (a skill load reads as a skill, everything else as generic
 * system context); a run of adjacent turns collapses into one divider that shows
 * the count and expands to each segment, self-labelled, in order. Same visual
 * language as `CompactSummaryBlock`. Expands to the full markdown on click.
 */
export function MetaFoldBlock({ segments, forceOpen }: Props) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(!!forceOpen);
  useEffect(() => setOpen(!!forceOpen), [forceOpen]);

  const total = segments.reduce((n, s) => n + s.length, 0);
  const merged = segments.length > 1;
  const { title, tag } = merged
    ? {
        title: t("detail.codex_meta", "系统上下文"),
        tag: t("detail.codex_meta_count", "{{count}} 条", { count: segments.length }),
      }
    : deriveHeader(segments[0] ?? "", t);

  return (
    <div className={styles.root}>
      <button className={styles.header} onClick={() => setOpen((o) => !o)}>
        <span className={styles.line} />
        <span className={styles.label}>
          <span className={styles.icon}>⚙</span>
          {title}
          <span className={styles.slug}>{tag}</span>
          <span className={styles.size}>· {formatSize(total)}</span>
          <span className={styles.arrow}>{open ? "▾" : "▸"}</span>
        </span>
        <span className={styles.line} />
      </button>
      {open && (
        <div className={styles.body}>
          {segments.map((seg, i) => (
            <div key={i} className={merged ? styles.segment : undefined}>
              {merged && (
                <div className={styles.segment_head}>
                  {deriveHeader(seg, t).tag}
                  <span className={styles.size}>· {formatSize(seg.length)}</span>
                </div>
              )}
              <TextBlock text={seg} />
            </div>
          ))}
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
  if (head.startsWith("<environment_context>"))
    return { title, tag: t("detail.codex_meta_environment", "环境上下文") };
  if (head.startsWith("<user_instructions>"))
    return { title, tag: t("detail.codex_meta_instructions", "用户指令") };
  if (head.startsWith("<turn_aborted>"))
    return { title, tag: t("detail.codex_meta_turn_aborted", "轮次中断") };
  if (head.startsWith("<subagent_notification>"))
    return { title, tag: t("detail.codex_meta_subagent", "子智能体通知") };
  return { title, tag: t("detail.codex_meta_generic", "注入指令") };
}
