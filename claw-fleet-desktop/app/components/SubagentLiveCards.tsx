import { useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { Copy, FileText, FolderOpen, PanelRightOpen } from "lucide-react";
import type { TFunction } from "i18next";

import { canRevealPath } from "../canReveal";
import type { SessionInfo } from "../types";
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
 * A live subagent's right-click menu.
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
  onOpen: (s: SessionInfo) => void,
  t: TFunction,
): ContextMenuItem[] {
  const copy = (text: string) => {
    writeText(text).catch((e) => console.error("clipboard write failed:", e));
  };
  const items: ContextMenuItem[] = [
    {
      id: "open",
      label: t("history.menu_open", "打开"),
      icon: <PanelRightOpen size={13} strokeWidth={1.7} />,
      onSelect: () => onOpen(a),
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
 * The card reads the fields that answer *is this one healthy and what is it
 * costing*: which model and effort it was given, how long it has been running,
 * its token rate and its spend. All of them were already on `SessionInfo` and
 * none of them were shown — the card used to carry a type, a status, a title
 * and a preview, which says what an agent is but nothing about how it is doing.
 *
 * Renders bare cards, no container: the rail owns the stack (and its scroll),
 * because the doc cards below these are siblings in one column, not a second
 * section under a divider.
 */
export function SubagentLiveCards({
  agents,
  onOpen,
}: {
  /** Live subagents, most-recently-active first. */
  agents: SessionInfo[];
  onOpen: (session: SessionInfo) => void;
}) {
  const { t } = useTranslation();
  const [menu, setMenu] = useState<{ anchor: ContextMenuAnchor; agent: SessionInfo } | null>(null);
  if (agents.length === 0) return null;
  const shown = agents.slice(0, LIVE_CARD_CAP);
  const hidden = agents.length - shown.length;

  return (
    <>
      {shown.map((a) => {
        const model = a.model ? formatModel(a.model) : "";
        const spend = a.totalCostUsd ?? 0;
        return (
          <button
            key={a.id}
            type="button"
            className={`${styles.rail_card} ${styles.agent_card}`}
            onClick={() => onOpen(a)}
            onContextMenu={(e) => {
              // Without this the app-wide menu (contextMenu.ts) answers with
              // Settings / About / Quit — see SessionAuxRail's note.
              e.preventDefault();
              e.stopPropagation();
              setMenu({ anchor: { x: e.clientX, y: e.clientY }, agent: a });
            }}
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
            {/* What it was given to work with. Dim and mono, one line, because
                it never changes once the agent starts — unlike everything on the
                meta row below, which is why the two are separate rows. */}
            {(model || a.effort) && (
              <div className={styles.agent_card_spec}>
                {model}
                {model && a.effort ? " · " : ""}
                {a.effort ?? ""}
              </div>
            )}
            {/* Latest activity — the same preview the session cards show, which
                is what makes this a live card rather than a name tag. */}
            {a.lastMessagePreview && (
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
          items={agentMenuItems(menu.agent, onOpen, t)}
          onClose={() => setMenu(null)}
        />
      )}
    </>
  );
}
