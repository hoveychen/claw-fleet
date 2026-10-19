import { FileText, Globe, NotebookText, Package } from "lucide-react";
import { Fragment, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import type { PointerEvent as ReactPointerEvent } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";

import { revealSlugInWikiPage } from "../hooks/useWikiDocs";
import { useUIStore } from "../store";
import {
  auxDocMeta,
  explainCardId,
  orderRailItems,
  type AuxDoc,
  type AuxDocKind,
} from "../detailAux";
import type { ExplainRecord } from "../explainApi";
import { groupExplainThreads } from "../selectionExplain";
import type { PathLinkContext } from "../markdown/pathLinks";
import type { SessionInfo } from "../types";
import { buildChipMenu, type AuxCardTail } from "./auxDocMenu";
import { ContextMenu, type ContextMenuAnchor, type ContextMenuItem } from "./ContextMenu";
import { SessionAuxAgent } from "./SessionAuxAgent";
import { SessionAuxDoc } from "./SessionAuxDoc";
import { SessionAuxExplain } from "./SessionAuxExplain";
import { SubagentLiveCards } from "./SubagentLiveCards";
import styles from "./SessionDetail.module.css";
import { Presence } from "./Presence";
import { InReaderHost } from "./readerHost";
import railStyles from "./TaskRail.module.css";

const DOC_ICON: Record<AuxDocKind, typeof FileText> = {
  file: FileText,
  wiki: NotebookText,
  web: Globe,
  // The same glyph the Outputs page uses for itself (its empty state).
  artifact: Package,
};

/**
 * The ambient sections of the task rail: each subagent running right now, each
 * file / wiki doc / page / deliverable the agent named that the reader opened,
 * and each side question asked about the prose. Rendered inside TaskRail's
 * column as list rows, one section per kind, each omitted while empty.
 *
 * These used to be raised cards floating over the transcript's right side, with
 * TaskRail as a second column beside them — two right-hand surfaces, one with
 * no switch, and the facet drawer offset for only one of them. They are one
 * column now, and the header switch owns all of it.
 *
 * **Reading happens beside the rail, not in it.** The column is too narrow to
 * read a file or a transcript in, so an expanded row stays in place (marked
 * open) and portals its reader into the host SessionDetail floats over the
 * conversation's right edge (see readerHost). The conversation holds that
 * reader's width clear (`--rail-band`, set by SessionDetail), so the reader
 * never covers prose. Its width is a persisted ratio of the pane (see
 * useDocCardWidth), dragged from the grip on its left edge.
 *
 * **A subagent card expands too**, into its transcript, rather than navigating
 * to the subagent's own session view — which cost you the conversation you
 * were reading. See SubagentLiveCards.
 *
 * **An expanded doc reader has no separate title strip.** Its own `AuxDocBar`
 * is the header — it already carries the name, so a strip above it printed
 * that name a second time. The bar's subject doubles as the collapse control
 * and so does its trailing ✕: closing a reader never takes the row out of the
 * rail, which is a near-permanent column now. (An agent reader keeps its strip: its
 * pane is a transcript, which has no header of its own to borrow.)
 *
 * **Every row answers a right-click** with its own menu (`auxDocMenu`), and
 * they must: with no handler, the right-click bubbled to the app-wide menu in
 * `contextMenu.ts` and answered a request to act on a file with
 * Settings / About / Quit.
 */
export function SessionAuxRail({
  agents,
  docs,
  explains,
  expandedId,
  workspacePath,
  onOpenAgent,
  onToggleAgent,
  onCloseAgent,
  onToggleDoc,
  onCloseDoc,
  onCloseOtherDocs,
  onCloseAllDocs,
  onHideRail,
  onOpenWiki,
  onToggleExplain,
  onCloseExplain,
  onLocateExplain,
  onFollowUpExplain,
  paths,
  onGripDown,
}: {
  /** Live subagents, most-recently-active first. */
  agents: SessionInfo[];
  docs: AuxDoc[];
  /** Side questions asked about this session's prose, oldest first. */
  explains: ExplainRecord[];
  /** The card expanded into a reader — a doc, an agent or a side question — if any. */
  expandedId: string | null;
  /** The session's repo, for the file card's open-in-repo action. */
  workspacePath: string;
  /** Leave for the subagent's own session view (the expanded card's ↗). */
  onOpenAgent: (session: SessionInfo) => void;
  /** Expand a subagent's transcript here, or collapse the open one. */
  onToggleAgent: (session: SessionInfo) => void;
  /** Dismiss a subagent's preview, and its card when it was only pinned. */
  onCloseAgent: (session: SessionInfo) => void;
  /** Expand a card, or collapse the one already expanded. */
  onToggleDoc: (id: string) => void;
  onCloseDoc: (id: string) => void;
  /** Keep this card, dismiss the rest — the rail caps at 8 and fills up. */
  onCloseOtherDocs: (id: string) => void;
  onCloseAllDocs: () => void;
  /** The header switch's own action, offered here too so the rail can be put
   *  away from the thing you want gone rather than from across the header. */
  onHideRail: () => void;
  /** A `[[slug]]` followed from inside a wiki doc opens the next one. */
  onOpenWiki: (slug: string) => void;
  /** Expand a side-question chain into the full exchange, or collapse it.
   *  The id is the chain's root record — one card, one id. */
  onToggleExplain: (id: string) => void;
  /** Hide a side-question card for this view; the records stay on disk. Takes
   *  every record in the chain, since the card is the whole chain. */
  onCloseExplain: (ids: string[]) => void;
  /** Scroll back to and re-select the passage a side question quoted. */
  onLocateExplain: (rec: ExplainRecord) => void;
  /** Ask a follow-up on a settled side question. */
  onFollowUpExplain: (rec: ExplainRecord, question: string) => void;
  /** Workspace context for path chips inside an expanded agent transcript. */
  paths?: PathLinkContext;
  onGripDown: (e: ReactPointerEvent<HTMLElement>) => void;
}) {
  const { t } = useTranslation();
  const requestFileNav = useUIStore((s) => s.requestFileNav);
  const requestArtifactNav = useUIStore((s) => s.requestArtifactNav);
  // One menu at a time for the whole rail.
  const [menu, setMenu] = useState<{ anchor: ContextMenuAnchor; items: ContextMenuItem[] } | null>(
    null,
  );
  // One card per *chain*, not per record: a follow-up is its own record, so
  // otherwise a three-turn conversation spent three of the rail's ten slots
  // and read bottom-up. A chain's place in the stack is its newest turn, so
  // following one up brings it back to the top.
  const explainThreads = useMemo(
    () =>
      groupExplainThreads(explains).map((th) => ({
        id: th.id,
        createdMs: th.records.reduce((max, r) => Math.max(max, r.createdMs), 0),
        records: th.records,
      })),
    [explains],
  );
  const stack = orderRailItems(docs, explainThreads, expandedId);

  const tailFor = (d: AuxDoc): AuxCardTail => ({
    isExpanded: d.id === expandedId,
    onToggle: () => onToggleDoc(d.id),
    onClose: () => onCloseDoc(d.id),
    onCloseOthers: () => onCloseOtherDocs(d.id),
    onCloseAll: onCloseAllDocs,
    onHideRail,
    otherCount: docs.length - 1,
  });

  /** The open reader's tail: its ✕ (and its menu's close item) only collapses
   *  the card back into the rail. Taking a doc out of the rail is the chip's
   *  right-click menu, and deleting a deliverable still does it (`onRemove`). */
  const readerTailFor = (d: AuxDoc): AuxCardTail => ({
    ...tailFor(d),
    onClose: () => onToggleDoc(d.id),
    onRemove: () => onCloseDoc(d.id),
  });

  /** Where a chip's "open in..." action sends this kind. Absent when the destination needs
   *  something this rail does not have (a file with no workspace). */
  const openPageFor = (d: AuxDoc): (() => void) | undefined => {
    switch (d.kind) {
      case "file":
        return workspacePath
          ? () => requestFileNav({ workspacePath, absPath: d.ref, line: null })
          : undefined;
      case "wiki":
        return () => revealSlugInWikiPage(d.ref);
      case "artifact":
        return () => requestArtifactNav(d.ref);
      case "web":
        return () => {
          openUrl(d.ref).catch((e) => console.error("openUrl failed:", d.ref, e));
        };
    }
  };

  const openChipMenu = (d: AuxDoc, e: React.MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setMenu({
      anchor: { x: e.clientX, y: e.clientY },
      items: buildChipMenu({
        doc: d,
        tail: tailFor(d),
        t,
        // A chip has nowhere to print an error, and the menu has closed by the
        // time an invoke rejects — the console is the honest destination.
        fail: (m) => console.error("aux chip action failed:", m),
        onOpenPage: openPageFor(d),
      }),
    });
  };

  const renderDoc = (d: AuxDoc) => {
    const Icon = DOC_ICON[d.kind];
    const isOpen = d.id === expandedId;
    const meta = auxDocMeta(d.kind, d.ref);
    return (
      <Fragment key={d.id}>
        <div
          className={`${styles.rail_card} ${styles.doc_card} ${isOpen ? styles.rail_card_open : ""}`}
          onContextMenu={(e) => openChipMenu(d, e)}
        >
          <button
            type="button"
            className={styles.doc_card_main}
            onClick={() => onToggleDoc(d.id)}
            title={d.ref}
            aria-expanded={isOpen}
          >
            <Icon
              className={styles.doc_card_icon}
              data-kind={d.kind}
              size={13}
              strokeWidth={1.8}
              aria-hidden="true"
            />
            <span className={styles.doc_card_label}>{d.label}</span>
            {/* One value, mono, dim: enough to tell two chips of the same
                kind apart (which poster, which version) without making the
                chip a second line tall. */}
            {meta && <span className={styles.doc_card_meta}>{meta}</span>}
          </button>
        </div>
        {isOpen && (
          <InReaderHost>
            <div className={styles.doc_card_expanded}>
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
              {/* No title strip above the reader: its own AuxDocBar is the
                  header (see the note on this component), and that bar carries
                  both the collapse and the ✕. */}
              <div className={styles.aux_doc_pane}>
                <SessionAuxDoc
                  doc={d}
                  tail={readerTailFor(d)}
                  workspacePath={workspacePath}
                  onOpenWiki={onOpenWiki}
                />
              </div>
            </div>
          </InReaderHost>
        )}
      </Fragment>
    );
  };

  const docEntries = stack.flatMap((e) => (e.type === "doc" ? [e.item] : []));
  const explainEntries = stack.flatMap((e) => (e.type === "explain" ? [e.item] : []));

  // Sections of the task rail, one per kind, each omitted while empty. Order
  // within a section is the stack's recency order (newest first).
  return (
    <>
      {agents.length > 0 && (
        <section className={railStyles.block}>
          <h3 className={railStyles.head}>
            {t("home.rail_agents", "Subagents")}
            <span className={railStyles.count}>{agents.length}</span>
          </h3>
          <div className={railStyles.card}>
            <SubagentLiveCards
              agents={agents}
              expandedId={expandedId}
              onToggle={onToggleAgent}
              onClose={onCloseAgent}
              onGoto={onOpenAgent}
              onHideRail={onHideRail}
              onGripDown={onGripDown}
              renderPane={(a) => <SessionAuxAgent agent={a} paths={paths} />}
            />
          </div>
        </section>
      )}
      {docEntries.length > 0 && (
        <section className={railStyles.block}>
          <h3 className={railStyles.head}>
            {t("home.rail_docs", "Opened")}
            <span className={railStyles.count}>{docEntries.length}</span>
          </h3>
          <div className={railStyles.card}>{docEntries.map(renderDoc)}</div>
        </section>
      )}
      {explainEntries.length > 0 && (
        <section className={railStyles.block}>
          <h3 className={railStyles.head}>
            {t("home.rail_explains", "Side questions")}
            <span className={railStyles.count}>{explainEntries.length}</span>
          </h3>
          <div className={railStyles.card}>
            {explainEntries.map((th) => {
              const last = th.records[th.records.length - 1];
              return (
                <SessionAuxExplain
                  key={th.id}
                  records={th.records}
                  isOpen={explainCardId(th.id) === expandedId}
                  onToggle={() => onToggleExplain(th.id)}
                  onClose={() => onCloseExplain(th.records.map((r) => r.id))}
                  onLocate={() => onLocateExplain(th.records[0])}
                  onFollowUp={(q) => onFollowUpExplain(last, q)}
                  onGripDown={onGripDown}
                  onHideRail={onHideRail}
                />
              );
            })}
          </div>
        </section>
      )}
      <Presence when={Boolean(menu)}>{menu && (
        <ContextMenu anchor={menu.anchor} items={menu.items} onClose={() => setMenu(null)} />
      )}</Presence>
    </>
  );
}
