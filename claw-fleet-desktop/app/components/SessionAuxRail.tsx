import { FileText, Globe, NotebookText } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { AuxDoc, AuxDocKind } from "../detailAux";
import type { SessionInfo } from "../types";
import { SubagentLiveCards } from "./SubagentLiveCards";
import styles from "./SessionDetail.module.css";

const DOC_ICON: Record<AuxDocKind, typeof FileText> = {
  file: FileText,
  wiki: NotebookText,
  web: Globe,
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
 * clear of the band the cards occupy.
 *
 * Visibility is controlled by `open`, whose default in SessionDetail follows
 * the content: nothing in play means `null` and zero width rather than an empty
 * frame held open. That is the whole reason it can be permanent — it is only
 * ever there when it has something to say, or when the reader pinned it open
 * with the header's switch.
 *
 * Clicking a doc card reads it in the drawer (`SessionAuxPanel`) at full width —
 * the rail is the inventory, the drawer is the reader. Clicking a subagent card
 * navigates to that subagent's transcript, the same as it always did.
 */
export function SessionAuxRail({
  open,
  agents,
  docs,
  activeId,
  onOpenAgent,
  onOpenDoc,
  onCloseDoc,
}: {
  /** Whether the column is on screen at all. The header's switch owns this;
   *  its default follows the content (see SessionDetail's `railOpen`). */
  open: boolean;
  /** Live subagents, most-recently-active first. */
  agents: SessionInfo[];
  docs: AuxDoc[];
  /** What the drawer is showing, so its card can say so. */
  activeId: string | null;
  onOpenAgent: (session: SessionInfo) => void;
  onOpenDoc: (id: string) => void;
  onCloseDoc: (id: string) => void;
}) {
  const { t } = useTranslation();
  if (!open) return null;
  const empty = agents.length === 0 && docs.length === 0;
  // No drag region on the <aside>: the column is pointer-events:none between
  // the cards so the transcript underneath keeps the wheel and the clicks.
  return (
    <aside className={styles.rail}>
      {/* Held open by the switch with nothing in it. Saying so beats an empty
          column, which reads as the rail having failed to load rather than as
          "there is genuinely nothing running and nothing opened yet". */}
      {empty && <p className={styles.rail_empty}>{t("detail.rail_empty", "暂无运行中的 Agent 或已打开的文档")}</p>}
      <SubagentLiveCards agents={agents} onOpen={onOpenAgent} />
      {/* Newest first: the file the agent just named is the one you are most
          likely to be reaching for, and it lands nearest the live agents. */}
      {[...docs].reverse().map((d) => {
        const Icon = DOC_ICON[d.kind];
        return (
          <div
            key={d.id}
            className={`${styles.rail_card} ${styles.doc_card} ${
              activeId === d.id ? styles.doc_card_open : ""
            }`}
          >
            <button
              type="button"
              className={styles.doc_card_main}
              onClick={() => onOpenDoc(d.id)}
              title={d.ref}
            >
              <Icon className={styles.doc_card_icon} size={13} strokeWidth={1.8} aria-hidden="true" />
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
          </div>
        );
      })}
    </aside>
  );
}
