import { ChevronDown, FileText, Globe, NotebookText, Package } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { PointerEvent as ReactPointerEvent } from "react";
import { agentCardId, type AuxDoc, type AuxDocKind } from "../detailAux";
import type { PathLinkContext } from "../markdown/pathLinks";
import type { SessionInfo } from "../types";
import { SessionAuxAgent } from "./SessionAuxAgent";
import { SessionAuxDoc } from "./SessionAuxDoc";
import { SubagentLiveCards } from "./SubagentLiveCards";
import styles from "./SessionDetail.module.css";

const DOC_ICON: Record<AuxDocKind, typeof FileText> = {
  file: FileText,
  wiki: NotebookText,
  web: Globe,
  // The same glyph the 产出 page uses for itself (its empty state).
  artifact: Package,
};

/**
 * The auxiliary rail — the *ambient* half of the auxiliary surfaces.
 *
 * One rounded, raised card per thing currently in play: each subagent running
 * right now, and each file / wiki doc / page the agent named that the reader
 * opened. No tabs, no headings, no dividers — a card's own edge and shadow is
 * the only separation it needs, and "how many are there" is answered by
 * counting shapes rather than reading a strip.
 *
 * The cards *float over* the transcript's right side (absolutely positioned
 * inside the messages pane) rather than filling a column beside it. They used
 * to be a second slab in the row, which read as a separate window and — the
 * tell — left the transcript's scrollbar stranded in the middle of the pane
 * with another surface to the right of it. The conversation now keeps the whole
 * pane and its scrollbar keeps the right edge; the reading column just holds
 * clear of the band the *collapsed* cards occupy.
 *
 * **Reading happens here too.** Clicking a doc card expands it in place into a
 * wide floating card that carries the same three readers the 仓库 / 知识库
 * pages use. It used to open the drawer instead, which put the doc's name on
 * screen twice (the drawer's title and the card that had just registered it)
 * and threw a fixed 560px panel over a conversation that, in a narrow pane, was
 * left with a sliver. The drawer is now facet-only.
 *
 * The expanded card is the only thing that grows: the rail's box widens to hold
 * it and the collapsed cards stay `--rail-w`, keeping the right edge. The
 * conversation widens its reserved band to match (`--rail-band`, set by
 * SessionDetail from the same number), so the card never covers prose — the
 * band is padding *inside* the scroller, which is what keeps the transcript's
 * scrollbar on the pane's own right edge. Its width is a persisted ratio of the
 * pane (see useDocCardWidth), dragged from the grip on its left edge.
 *
 * **A subagent card expands here too**, into its transcript. It used to
 * navigate to the subagent's own session view, which cost you the conversation
 * you were reading — for a thing whose only content is its messages. See
 * SubagentLiveCards.
 */
export function SessionAuxRail({
  open,
  agents,
  docs,
  expandedId,
  onOpenAgent,
  onToggleAgent,
  onCloseAgent,
  onToggleDoc,
  onCloseDoc,
  onOpenWiki,
  paths,
  cardWidth,
  onGripDown,
}: {
  /** Whether the column is on screen at all. The header's switch owns this;
   *  its default follows the content (see SessionDetail's `railOpen`). */
  open: boolean;
  /** Live subagents, most-recently-active first. */
  agents: SessionInfo[];
  docs: AuxDoc[];
  /** The card expanded into a reader — a doc or an agent — if any. */
  expandedId: string | null;
  /** Leave for the subagent's own session view (the expanded card's ↗). */
  onOpenAgent: (session: SessionInfo) => void;
  /** Expand a subagent's transcript here, or collapse the open one. */
  onToggleAgent: (session: SessionInfo) => void;
  /** Dismiss a subagent's preview, and its card when it was only pinned. */
  onCloseAgent: (session: SessionInfo) => void;
  /** Expand a card, or collapse the one already expanded. */
  onToggleDoc: (id: string) => void;
  onCloseDoc: (id: string) => void;
  /** A `[[slug]]` followed from inside a wiki doc opens the next one. */
  onOpenWiki: (slug: string) => void;
  /** Workspace context for path chips inside an expanded agent transcript. */
  paths?: PathLinkContext;
  /** px width for the expanded card — owned by SessionDetail because the
   *  conversation has to reserve the same number. 0 when nothing is expanded. */
  cardWidth: number;
  onGripDown: (e: ReactPointerEvent<HTMLElement>) => void;
}) {
  const { t } = useTranslation();
  const expandedDoc = docs.find((d) => d.id === expandedId) ?? null;

  if (!open) return null;
  const empty = agents.length === 0 && docs.length === 0;
  // Either kind of expansion widens the box — a transcript needs the reading
  // width a file does. Checked against the cards actually in hand, so a stale
  // id (its agent retired and unpinned) cannot widen the rail around nothing.
  const expandedAgent = agents.some((a) => agentCardId(a.id) === expandedId);
  const wide = cardWidth > 0 && (expandedDoc != null || expandedAgent);
  // No drag region on the <aside>: the column is pointer-events:none between
  // the cards so the transcript underneath keeps the wheel and the clicks.
  return (
    <aside
      className={`${styles.rail} ${wide ? styles.rail_wide : ""}`}
      style={wide ? { width: cardWidth } : undefined}
    >
      {/* Held open by the switch with nothing in it. Saying so beats an empty
          column, which reads as the rail having failed to load rather than as
          "there is genuinely nothing running and nothing opened yet". */}
      {empty && <p className={styles.rail_empty}>{t("detail.rail_empty", "暂无运行中的 Agent 或已打开的文档")}</p>}
      <SubagentLiveCards
        agents={agents}
        expandedId={expandedId}
        onToggle={onToggleAgent}
        onClose={onCloseAgent}
        onGoto={onOpenAgent}
        onGripDown={onGripDown}
        renderPane={(a) => <SessionAuxAgent agent={a} paths={paths} />}
      />
      {/* Newest first: the file the agent just named is the one you are most
          likely to be reaching for, and it lands nearest the live agents. */}
      {[...docs].reverse().map((d) => {
        const Icon = DOC_ICON[d.kind];
        const isOpen = d.id === expandedId;
        const head = (
          <>
            <button
              type="button"
              className={styles.doc_card_main}
              onClick={() => onToggleDoc(d.id)}
              title={d.ref}
              aria-expanded={isOpen}
            >
              {isOpen ? (
                <ChevronDown className={styles.doc_card_icon} size={13} strokeWidth={1.8} aria-hidden="true" />
              ) : (
                <Icon className={styles.doc_card_icon} size={13} strokeWidth={1.8} aria-hidden="true" />
              )}
              <span className={styles.doc_card_label}>{d.label}</span>
            </button>
            <button
              type="button"
              className={styles.doc_card_close}
              onClick={() => onCloseDoc(d.id)}
              title={t("common.close", "关闭")}
              aria-label={t("common.close", "关闭")}
            >
              ✕
            </button>
          </>
        );
        if (!isOpen) {
          return (
            <div key={d.id} className={`${styles.rail_card} ${styles.doc_card}`}>
              {head}
            </div>
          );
        }
        return (
          <div key={d.id} className={`${styles.rail_card} ${styles.doc_card_expanded}`}>
            {/* Grabbed from the card's left edge — the edge that moves, since
                the card grows toward the conversation. `separator` with an
                orientation is what a resize grip is. */}
            <div
              className={styles.doc_card_grip}
              onPointerDown={onGripDown}
              role="separator"
              aria-orientation="vertical"
              aria-label={t("detail.doc_card_resize", "调整卡片宽度")}
            />
            <div className={`${styles.doc_card} ${styles.doc_card_head}`}>{head}</div>
            <SessionAuxDoc doc={d} onOpenWiki={onOpenWiki} onClose={() => onCloseDoc(d.id)} />
          </div>
        );
      })}
    </aside>
  );
}
