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
 * A permanent column beside the conversation holding one rounded, raised card
 * per thing currently in play: each subagent running right now, and each file /
 * wiki doc / page the agent named that the reader opened. No tabs, no headings,
 * no dividers — the cards sit directly on the recessed ground between the two
 * slabs, so their own edge and shadow is the only separation they need, and
 * "how many are there" is answered by counting shapes rather than reading a
 * strip.
 *
 * Renders `null` when there is nothing in play, which is what makes the column
 * cost zero width rather than holding an empty frame open. That is the whole
 * reason it can be permanent: it is only ever there when it has something to
 * say.
 *
 * Clicking a doc card reads it in the drawer (`SessionAuxPanel`) at full width —
 * the rail is the inventory, the drawer is the reader. Clicking a subagent card
 * navigates to that subagent's transcript, the same as it always did.
 */
export function SessionAuxRail({
  agents,
  docs,
  activeId,
  onOpenAgent,
  onOpenDoc,
  onCloseDoc,
}: {
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
  if (agents.length === 0 && docs.length === 0) return null;
  return (
    <aside className={styles.rail} data-tauri-drag-region>
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
