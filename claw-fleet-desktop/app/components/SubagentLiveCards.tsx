import { ChevronDown, ExternalLink } from "lucide-react";
import type { PointerEvent as ReactPointerEvent, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { agentCardId } from "../detailAux";
import { isLiveMember, type SessionInfo } from "../types";
import { StatusBadge } from "./SessionCard";
import { timeAgo } from "./SessionRow";
import styles from "./SessionDetail.module.css";

/** How many cards render before the rest collapse into a "+N" line. A workflow
 *  fan-out can put a hundred agents in flight at once; the panel is meant to be
 *  read at a glance, and the Workflow facet is where the full DAG lives. */
export const LIVE_CARD_CAP = 6;

/**
 * The subagents this session has in flight, one card each, at the top of the
 * auxiliary rail.
 *
 * Before this, a running subagent was only visible if you went looking: the
 * scope dropdown in the header (which navigates *away* from the parent) or the
 * 后台任务 tab (a last-Stop snapshot, minutes stale for a subagent). Neither
 * answered "what is everything working on right now" without clicking. These
 * cards do, and they disappear the moment the last one finishes — the rail is a
 * picture of what is live, not a log.
 *
 * **Clicking one expands it in place into its transcript** (`renderPane`),
 * exactly as a doc card expands into its reader. It used to navigate to the
 * subagent's own session view, which cost you the conversation you were reading
 * and a trip back — for a thing whose only content *is* its messages. Going
 * there is still one click, from the expanded card's ↗ button.
 *
 * Renders bare cards, no container: the rail owns the stack (and its scroll),
 * because the doc cards below these are siblings in one column, not a second
 * section under a divider.
 */
export function SubagentLiveCards({
  agents,
  expandedId,
  onToggle,
  onClose,
  onGoto,
  onGripDown,
  renderPane,
}: {
  /** Subagents to card, most-recently-active first. Live ones, plus at most
   *  one finished agent the reader pinned by opening it (see `pinnedAgent`). */
  agents: SessionInfo[];
  /** The expanded card's id, doc or agent. Only an `agent:` id matches here. */
  expandedId: string | null;
  /** Expand this agent's transcript, or collapse the one already expanded. */
  onToggle: (session: SessionInfo) => void;
  /** Dismiss the preview — and the card itself, when it is only still in the
   *  rail because it was pinned. */
  onClose: (session: SessionInfo) => void;
  /** Leave for the subagent's own session view. The escape hatch, not the
   *  default: everything the page adds over this pane is composer and chrome a
   *  subagent has no use for. */
  onGoto: (session: SessionInfo) => void;
  onGripDown: (e: ReactPointerEvent<HTMLElement>) => void;
  /** The transcript pane for the expanded card. Supplied by the rail so this
   *  component stays free of the fetching. */
  renderPane: (session: SessionInfo) => ReactNode;
}) {
  const { t } = useTranslation();
  if (agents.length === 0) return null;
  const shown = agents.slice(0, LIVE_CARD_CAP);
  const hidden = agents.length - shown.length;

  return (
    <>
      {shown.map((a) => {
        const isOpen = expandedId === agentCardId(a.id);
        // A card the scan no longer lists as live is here only because the
        // reader pinned it — so it, unlike a live one, is dismissible.
        const pinned = !isLiveMember(a);
        const head = (
          <>
            <button
              type="button"
              className={styles.agent_card_main}
              onClick={() => onToggle(a)}
              title={t("detail.bgtask_open_hint")}
              aria-expanded={isOpen}
            >
              <div className={styles.agent_card_head}>
                {isOpen && (
                  <ChevronDown
                    className={styles.doc_card_icon}
                    size={13}
                    strokeWidth={1.8}
                    aria-hidden="true"
                  />
                )}
                <span className={styles.agent_card_type}>
                  {a.agentType ?? t("detail.live_agent_generic", "Agent")}
                </span>
                <StatusBadge status={a.status} />
              </div>
              {/* Identity: the agent's own title if the scan inferred one, else
                  the description the parent gave the Task tool. */}
              <div className={styles.agent_card_title}>
                {a.aiTitle || a.agentDescription || a.id}
              </div>
              {/* Latest activity — the same preview the session cards show,
                  which is what makes this a live card rather than a name tag.
                  Redundant once the transcript itself is on screen. */}
              {!isOpen && a.lastMessagePreview && (
                <div className={styles.agent_card_preview}>{a.lastMessagePreview}</div>
              )}
              <div className={styles.agent_card_meta}>
                <span>{timeAgo(a.lastActivityMs, t)}</span>
                {a.agentTokenSpeed > 0 && <span>{Math.round(a.agentTokenSpeed)} tok/s</span>}
              </div>
            </button>
            {(isOpen || pinned) && (
              <div className={styles.agent_card_tools}>
                {isOpen && (
                  <button
                    type="button"
                    className={styles.agent_card_tool}
                    onClick={() => onGoto(a)}
                    title={t("detail.agent_card_goto", "在会话页打开")}
                    aria-label={t("detail.agent_card_goto", "在会话页打开")}
                  >
                    <ExternalLink size={12} strokeWidth={1.8} aria-hidden="true" />
                  </button>
                )}
                <button
                  type="button"
                  className={styles.agent_card_tool}
                  onClick={() => onClose(a)}
                  title={t("common.close", "关闭")}
                  aria-label={t("common.close", "关闭")}
                >
                  ✕
                </button>
              </div>
            )}
          </>
        );
        if (!isOpen) {
          return (
            <div key={a.id} className={`${styles.rail_card} ${styles.agent_card}`}>
              {head}
            </div>
          );
        }
        return (
          <div key={a.id} className={`${styles.rail_card} ${styles.doc_card_expanded}`}>
            {/* Same grip as a doc reader: the card grows toward the
                conversation, so its left edge is the one that moves. */}
            <div
              className={styles.doc_card_grip}
              onPointerDown={onGripDown}
              role="separator"
              aria-orientation="vertical"
              aria-label={t("detail.doc_card_resize", "调整卡片宽度")}
            />
            <div className={`${styles.agent_card} ${styles.doc_card_head}`}>{head}</div>
            {renderPane(a)}
          </div>
        );
      })}
      {hidden > 0 && (
        <div className={styles.agents_deck_more}>
          {t("detail.live_agents_more", { count: hidden })}
        </div>
      )}
    </>
  );
}
