import { useTranslation } from "react-i18next";
import {
  Bot,
  FileText,
  Globe,
  MessageCircleQuestion,
  NotebookText,
  Package,
  X,
} from "lucide-react";

import { auxDocMeta, type AuxDocKind } from "../detailAux";
import type { DocHistoryEntry } from "../docHistory";
import type { ExplainRecord } from "../explainApi";
import type { SessionInfo } from "../types";
import styles from "./SessionDetail.module.css";

const DOC_ICON: Record<AuxDocKind, typeof FileText> = {
  file: FileText,
  wiki: NotebookText,
  web: Globe,
  artifact: Package,
};

/**
 * Everything this session has put in the rail, in one list.
 *
 * The rail is a viewfinder — it shows what is in play and drops the rest, and
 * each of its three card kinds drops things differently: a live subagent card
 * cannot be dismissed at all, a doc card is gone at the ninth doc and again on
 * every session switch, a side question comes back however often it is closed.
 * Three cards with the same ✕ and three different meanings, and for two of
 * them the ✕ was indistinguishable from doing nothing.
 *
 * This panel is what makes one meaning possible: with a place that holds
 * everything, the rail's ✕ can mean "not in my way right now" for all three,
 * because nothing is ever lost by pressing it. Rows are grouped by kind and
 * ordered newest first; clicking one puts it back in the rail, expanded.
 *
 * Desktop-only, deliberately. The phone has no rail to fall out of — its side
 * questions live in a tab that always lists all of them
 * (`mobile-web/src/views/SessionExplainsTab.tsx`), and it has no doc cards at
 * all — so there is nothing there for this panel to recover.
 */
export function SessionLibraryPanel({
  explains,
  hiddenExplains,
  docs,
  subagents,
  onOpenExplain,
  onOpenDoc,
  onForgetDoc,
  onForgetAllDocs,
  onOpenAgent,
}: {
  /** Every side question on disk for this session, including ones dismissed
   *  from the rail. */
  explains: ExplainRecord[];
  /** Which of them are currently not in the rail — the rows worth a "put it
   *  back" affordance rather than "scroll to it". */
  hiddenExplains: ReadonlySet<string>;
  /** Everything opened in this session, past the rail's cap and across
   *  restarts. */
  docs: DocHistoryEntry[];
  /** Subagents of this session family, finished ones included. */
  subagents: SessionInfo[];
  onOpenExplain: (id: string) => void;
  onOpenDoc: (kind: AuxDocKind, ref: string, label: string) => void;
  onForgetDoc: (kind: AuxDocKind, ref: string) => void;
  onForgetAllDocs: () => void;
  onOpenAgent: (session: SessionInfo) => void;
}) {
  const { t } = useTranslation();
  const empty = explains.length === 0 && docs.length === 0 && subagents.length === 0;

  if (empty) {
    return (
      <div className={styles.library_panel}>
        <p className={styles.library_empty}>
          {t(
            "detail.library_empty",
            "这个会话还没有打开过文档，也没有追问记录。边栏里出现过的东西都会留在这里。",
          )}
        </p>
      </div>
    );
  }

  return (
    <div className={styles.library_panel}>
      {explains.length > 0 && (
        <section className={styles.library_group}>
          <h4 className={styles.library_group_head}>
            {t("detail.library_explains", "追问")} <span>{explains.length}</span>
          </h4>
          {[...explains]
            .sort((a, b) => b.createdMs - a.createdMs)
            .map((r) => (
              <button
                key={r.id}
                type="button"
                className={styles.library_row}
                onClick={() => onOpenExplain(r.id)}
                title={r.quote}
              >
                <MessageCircleQuestion className={styles.library_icon} size={13} strokeWidth={1.8} />
                <span className={styles.library_label}>{r.question || r.quote}</span>
                {hiddenExplains.has(r.id) && (
                  <span className={styles.library_tag}>
                    {t("detail.library_dismissed", "已收起")}
                  </span>
                )}
              </button>
            ))}
        </section>
      )}

      {docs.length > 0 && (
        <section className={styles.library_group}>
          <h4 className={styles.library_group_head}>
            {t("detail.library_docs", "打开过的文档")} <span>{docs.length}</span>
            <button
              type="button"
              className={styles.library_group_action}
              onClick={onForgetAllDocs}
            >
              {t("detail.library_forget_all", "清空")}
            </button>
          </h4>
          {docs.map((d) => {
            const Icon = DOC_ICON[d.kind];
            const meta = auxDocMeta(d.kind, d.ref);
            return (
              <div key={`${d.kind}:${d.ref}`} className={styles.library_row_wrap}>
                <button
                  type="button"
                  className={styles.library_row}
                  onClick={() => onOpenDoc(d.kind, d.ref, d.label)}
                  title={d.ref}
                >
                  <Icon
                    className={styles.library_icon}
                    data-kind={d.kind}
                    size={13}
                    strokeWidth={1.8}
                  />
                  <span className={styles.library_label}>{d.label}</span>
                  {meta && <span className={styles.library_meta}>{meta}</span>}
                </button>
                {/* The only delete the reading list has. A doc dropped here is
                    gone for good — but it is one click in the transcript to
                    open it again, which is how it got here. */}
                <button
                  type="button"
                  className={styles.library_forget}
                  onClick={() => onForgetDoc(d.kind, d.ref)}
                  title={t("detail.library_forget", "从列表中移除")}
                  aria-label={t("detail.library_forget", "从列表中移除")}
                >
                  <X size={12} strokeWidth={2} />
                </button>
              </div>
            );
          })}
        </section>
      )}

      {subagents.length > 0 && (
        <section className={styles.library_group}>
          <h4 className={styles.library_group_head}>
            {t("detail.library_agents", "子 Agent")} <span>{subagents.length}</span>
          </h4>
          {subagents.map((s) => (
            <button
              key={s.id}
              type="button"
              className={styles.library_row}
              onClick={() => onOpenAgent(s)}
              title={s.aiTitle || s.id}
            >
              <Bot className={styles.library_icon} size={13} strokeWidth={1.8} />
              <span className={styles.tab_dot} data-status={s.status} />
              <span className={styles.library_label}>
                {s.aiTitle || s.agentType || s.id}
              </span>
            </button>
          ))}
        </section>
      )}
    </div>
  );
}
