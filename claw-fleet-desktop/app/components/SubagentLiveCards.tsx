import { useTranslation } from "react-i18next";
import type { SessionInfo } from "../types";
import { StatusBadge } from "./SessionCard";
import { timeAgo } from "./SessionRow";
import styles from "./SessionDetail.module.css";

/** How many cards render before the rest collapse into a "+N" line. A workflow
 *  fan-out can put a hundred agents in flight at once; the panel is meant to be
 *  read at a glance, and the Workflow facet is where the full DAG lives. */
export const LIVE_CARD_CAP = 6;

/**
 * The subagents this session has in flight, one card each, pinned to the top of
 * the auxiliary column.
 *
 * Before this, a running subagent was only visible if you went looking: the
 * scope dropdown in the header (which navigates *away* from the parent) or the
 * 后台任务 tab (a last-Stop snapshot, minutes stale for a subagent). Neither
 * answered "what is everything working on right now" without clicking. These
 * cards do, and they disappear the moment an agent finishes — the panel is a
 * picture of what is live, not a log.
 */
export function SubagentLiveCards({
  agents,
  heading,
  onOpen,
}: {
  /** Live subagents, most-recently-active first. */
  agents: SessionInfo[];
  /** Label the deck. Suppressed when the panel header already names it — i.e.
   *  when the cards are the only thing the panel holds. */
  heading: boolean;
  onOpen: (session: SessionInfo) => void;
}) {
  const { t } = useTranslation();
  if (agents.length === 0) return null;
  const shown = agents.slice(0, LIVE_CARD_CAP);
  const hidden = agents.length - shown.length;

  return (
    <div className={styles.agents_deck}>
      {heading && (
        <div className={styles.agents_deck_head}>
          {t("detail.live_agents", { count: agents.length })}
        </div>
      )}
      {shown.map((a) => (
        <button
          key={a.id}
          type="button"
          className={styles.agent_card}
          onClick={() => onOpen(a)}
          title={t("detail.bgtask_open_hint")}
        >
          <div className={styles.agent_card_head}>
            <span className={styles.agent_card_type}>
              {a.agentType ?? t("detail.live_agent_generic", "Agent")}
            </span>
            <StatusBadge status={a.status} />
          </div>
          {/* Identity: the agent's own title if the scan inferred one, else the
              description the parent gave the Task tool. */}
          <div className={styles.agent_card_title}>
            {a.aiTitle || a.agentDescription || a.id}
          </div>
          {/* Latest activity — the same preview the session cards show, which
              is what makes this a live card rather than a name tag. */}
          {a.lastMessagePreview && (
            <div className={styles.agent_card_preview}>{a.lastMessagePreview}</div>
          )}
          <div className={styles.agent_card_meta}>
            <span>{timeAgo(a.lastActivityMs, t)}</span>
            {a.agentTokenSpeed > 0 && (
              <span>{Math.round(a.agentTokenSpeed)} tok/s</span>
            )}
          </div>
        </button>
      ))}
      {hidden > 0 && (
        <div className={styles.agents_deck_more}>
          {t("detail.live_agents_more", { count: hidden })}
        </div>
      )}
    </div>
  );
}
