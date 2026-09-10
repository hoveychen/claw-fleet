import {
  ChevronDown,
  Copy,
  ExternalLink,
  FileText,
  FolderOpen,
  PanelRightClose,
  PanelRightOpen,
} from "lucide-react";
import { useState, type PointerEvent as ReactPointerEvent, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import type { TFunction } from "i18next";

import { canRevealPath } from "../canReveal";
import { agentCardId } from "../detailAux";
import { isLiveMember, type SessionInfo } from "../types";
import { ContextMenu, type ContextMenuAnchor, type ContextMenuItem } from "./ContextMenu";
import { formatModel, StatusBadge } from "./SessionCard";
import { timeAgo } from "./SessionRow";
import styles from "./SessionDetail.module.css";

/** How many cards render before the rest collapse into a "+N" line. A workflow
 *  fan-out can put a hundred agents in flight at once; the panel is meant to be
 *  read at a glance, and the Workflow facet is where the full DAG lives. */
export const LIVE_CARD_CAP = 6;

/** `2m 14s` — long enough to be exact, short enough for a 10px mono line. A
 *  running agent's *elapsed* time is the number that says whether it is making
 *  progress; `timeAgo(lastActivity)` on its own cannot (a healthy agent and a
 *  wedged one both read "just now" the moment they print anything). */
function elapsed(sinceMs: number): string {
  if (!sinceMs) return "";
  const sec = Math.max(0, Math.floor((Date.now() - sinceMs) / 1000));
  if (sec < 60) return `${sec}s`;
  const min = Math.floor(sec / 60);
  if (min < 60) return `${min}m ${sec % 60}s`;
  return `${Math.floor(min / 60)}h ${min % 60}m`;
}

/**
 * A subagent card's right-click menu.
 *
 * **No 停止 item, deliberately.** A subagent is driven by its parent's process
 * and has no signal of its own — `StopControl.canControl` is literally
 * `!s.isSubagent`, and `SessionInfo.pid` is the *parent's*, shared by every
 * session in that working directory. An item labelled "stop this agent" would
 * either do nothing or cancel the parent's whole turn, so the honest menu omits
 * it; interrupting the parent is done from the parent's own Stop control, where
 * it says what it is.
 */
function agentMenuItems(
  a: SessionInfo,
  isOpen: boolean,
  onToggle: (s: SessionInfo) => void,
  onGoto: (s: SessionInfo) => void,
  onHideRail: () => void,
  t: TFunction,
): ContextMenuItem[] {
  const copy = (text: string) => {
    writeText(text).catch((e) => console.error("clipboard write failed:", e));
  };
  const items: ContextMenuItem[] = [
    {
      id: "toggle",
      label: isOpen
        ? t("detail.aux_collapse_card", "收起此卡")
        : t("detail.aux_expand_card", "展开此卡"),
      icon: <ChevronDown size={13} strokeWidth={1.7} />,
      onSelect: () => onToggle(a),
    },
    {
      id: "goto",
      label: t("detail.agent_card_goto", "在会话页打开"),
      icon: <PanelRightOpen size={13} strokeWidth={1.7} />,
      onSelect: () => onGoto(a),
    },
    {
      id: "copy-id",
      label: t("detail.copy_session_id", "复制会话 ID"),
      icon: <Copy size={13} strokeWidth={1.7} />,
      sub: a.id,
      dividerBefore: true,
      onSelect: () => copy(a.id),
    },
  ];
  if (a.jsonlPath) {
    items.push({
      id: "copy-transcript",
      label: t("detail.aux_copy_transcript", "复制 transcript 路径"),
      icon: <FileText size={13} strokeWidth={1.7} />,
      sub: a.jsonlPath,
      onSelect: () => copy(a.jsonlPath),
    });
    if (canRevealPath()) {
      const revealKey =
        document.documentElement.getAttribute("data-platform") === "windows"
          ? "paths.reveal_in_explorer"
          : "paths.reveal_in_finder";
      items.push({
        id: "reveal",
        label: t(revealKey),
        icon: <FolderOpen size={13} strokeWidth={1.7} />,
        onSelect: () => {
          invoke("reveal_path", { path: a.jsonlPath }).catch((e) =>
            console.error("reveal_path failed:", e),
          );
        },
      });
    }
  }
  // The rail's own switch, offered here for the same reason the doc cards
  // offer it: the rail's transparent background cannot answer a right-click
  // (`.rail` is pointer-events:none), so the cards are the only reachable
  // place to put the rail away from.
  items.push({
    id: "hide-rail",
    label: t("detail.rail_hide", "收起辅助栏"),
    icon: <PanelRightClose size={13} strokeWidth={1.7} />,
    dividerBefore: true,
    onSelect: onHideRail,
  });
  return items;
}

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
 * The card reads the fields that answer *is this one healthy and what is it
 * costing*: which model and effort it was given, how long it has been running,
 * its token rate and its spend. All of them were already on `SessionInfo` and
 * none of them were shown — the card used to carry a type, a status, a title
 * and a preview, which says what an agent is but nothing about how it is doing.
 *
 * A right-click raises the card's own menu. Without one it bubbled to the
 * app-wide menu in `contextMenu.ts`, which answered a request to act on an
 * agent with Settings / About / Quit.
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
  onHideRail,
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
  /** Put the whole rail away — see `agentMenuItems`. */
  onHideRail: () => void;
  onGripDown: (e: ReactPointerEvent<HTMLElement>) => void;
  /** The transcript pane for the expanded card. Supplied by the rail so this
   *  component stays free of the fetching. */
  renderPane: (session: SessionInfo) => ReactNode;
}) {
  const { t } = useTranslation();
  const [menu, setMenu] = useState<{ anchor: ContextMenuAnchor; agent: SessionInfo } | null>(null);
  if (agents.length === 0) return null;
  // The one being read is never capped out. The list is sorted by activity, so
  // a fan-out of seven can push the agent you are reading past the cap between
  // two scan ticks — which would unmount its transcript while the rail stayed
  // widened around the hole where it had been.
  const openIdx = agents.findIndex((a) => agentCardId(a.id) === expandedId);
  const ordered =
    openIdx >= LIVE_CARD_CAP
      ? [agents[openIdx], ...agents.filter((_, i) => i !== openIdx)]
      : agents;
  const shown = ordered.slice(0, LIVE_CARD_CAP);
  const hidden = ordered.length - shown.length;

  /** Same menu whether the card is a chip or an expanded reader, and reachable
   *  from anywhere in it — including the transcript you are reading. */
  const onCardContextMenu = (a: SessionInfo) => (e: React.MouseEvent) => {
    if ((e.target as Element | null)?.closest?.("input, textarea, [contenteditable]")) return;
    e.preventDefault();
    e.stopPropagation();
    setMenu({ anchor: { x: e.clientX, y: e.clientY }, agent: a });
  };

  return (
    <>
      {shown.map((a) => {
        const isOpen = expandedId === agentCardId(a.id);
        // A card the scan no longer lists as live is here only because the
        // reader pinned it — so it, unlike a live one, is dismissible.
        const pinned = !isLiveMember(a);
        const model = a.model ? formatModel(a.model) : "";
        const spend = a.totalCostUsd ?? 0;
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
              {/* What it was given to work with. Fixed for the agent's whole
                  life, which is why it sits above the numbers that move — and
                  why, unlike the preview, it is worth keeping once the
                  transcript is open: the pane below names no model. */}
              {(model || a.effort) && (
                <div className={styles.agent_card_spec}>
                  {model}
                  {model && a.effort ? " · " : ""}
                  {a.effort ?? ""}
                </div>
              )}
              {/* Latest activity — the same preview the session cards show,
                  which is what makes this a live card rather than a name tag.
                  Redundant once the transcript itself is on screen. */}
              {!isOpen && a.lastMessagePreview && (
                <div className={styles.agent_card_preview}>{a.lastMessagePreview}</div>
              )}
              <div className={styles.agent_card_meta}>
                {/* Elapsed first: it is the one number that answers "is this
                    stuck", which is the question a live card exists for. */}
                {a.createdAtMs > 0 && <span>{elapsed(a.createdAtMs)}</span>}
                <span>{timeAgo(a.lastActivityMs, t)}</span>
                {a.agentTokenSpeed > 0 && <span>{Math.round(a.agentTokenSpeed)} tok/s</span>}
                {spend > 0 && <span>${spend.toFixed(2)}</span>}
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
            <div
              key={a.id}
              className={`${styles.rail_card} ${styles.agent_card}`}
              onContextMenu={onCardContextMenu(a)}
            >
              {head}
            </div>
          );
        }
        return (
          <div
            key={a.id}
            className={`${styles.rail_card} ${styles.doc_card_expanded}`}
            onContextMenu={onCardContextMenu(a)}
          >
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
      {menu && (
        <ContextMenu
          anchor={menu.anchor}
          items={agentMenuItems(
            menu.agent,
            expandedId === agentCardId(menu.agent.id),
            onToggle,
            onGoto,
            onHideRail,
            t,
          )}
          onClose={() => setMenu(null)}
        />
      )}
    </>
  );
}
