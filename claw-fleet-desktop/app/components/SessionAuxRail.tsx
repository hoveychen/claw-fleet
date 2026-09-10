import { ChevronsDownUp, FileText, Globe, NotebookText, Package, PanelRightClose, XCircle } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { PointerEvent as ReactPointerEvent } from "react";

import { openUrl } from "@tauri-apps/plugin-opener";
import { revealSlugInWikiPage } from "../hooks/useWikiDocs";
import { useUIStore } from "../store";
import { auxDocMeta, type AuxDoc, type AuxDocKind } from "../detailAux";
import type { SessionInfo } from "../types";
import { buildChipMenu, type AuxCardTail } from "./auxDocMenu";
import { ContextMenu, type ContextMenuAnchor, type ContextMenuItem } from "./ContextMenu";
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
 * right now, and each file / wiki doc / page / deliverable the agent named that
 * the reader opened. No tabs, no headings, no dividers — a card's own edge and
 * shadow is the only separation it needs, and "how many are there" is answered
 * by counting shapes rather than reading a strip.
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
 * wide floating card that carries the same readers the 仓库 / 知识库 / 产出
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
 * **The expanded card has no separate title strip.** Its reader's own
 * `AuxDocBar` is the header — it already carries the name, so a strip above it
 * printed that name a second time. The bar's subject doubles as the collapse
 * control and its trailing ✕ dismisses the card.
 *
 * **Everything here answers a right-click.** A collapsed chip, an expanded
 * card and the rail's own empty space each raise a menu (`auxDocMenu`), and
 * they must: with no handler, the right-click bubbled to the app-wide menu in
 * `contextMenu.ts` and answered a request to act on a file with
 * Settings / About / Quit.
 *
 * Clicking a subagent card navigates to that subagent's transcript, the same as
 * it always did.
 */
export function SessionAuxRail({
  open,
  agents,
  docs,
  expandedId,
  workspacePath,
  onOpenAgent,
  onToggleDoc,
  onCloseDoc,
  onCloseOtherDocs,
  onCloseAllDocs,
  onCollapseDoc,
  onHideRail,
  onOpenWiki,
  cardWidth,
  onGripDown,
}: {
  /** Whether the column is on screen at all. The header's switch owns this;
   *  its default follows the content (see SessionDetail's `railOpen`). */
  open: boolean;
  /** Live subagents, most-recently-active first. */
  agents: SessionInfo[];
  docs: AuxDoc[];
  /** The doc card expanded into a reader, if any. */
  expandedId: string | null;
  /** The session's repo, for the file card's 在仓库页打开. */
  workspacePath: string;
  onOpenAgent: (session: SessionInfo) => void;
  /** Expand a card, or collapse the one already expanded. */
  onToggleDoc: (id: string) => void;
  onCloseDoc: (id: string) => void;
  /** Keep this card, dismiss the rest — the rail caps at 8 and fills up. */
  onCloseOtherDocs: (id: string) => void;
  onCloseAllDocs: () => void;
  /** Collapse whatever is expanded, without dismissing it. */
  onCollapseDoc: () => void;
  /** The header switch's own action, offered here too so the rail can be put
   *  away from the thing you want gone rather than from across the header. */
  onHideRail: () => void;
  /** A `[[slug]]` followed from inside a wiki doc opens the next one. */
  onOpenWiki: (slug: string) => void;
  /** px width for the expanded card — owned by SessionDetail because the
   *  conversation has to reserve the same number. 0 when nothing is expanded. */
  cardWidth: number;
  onGripDown: (e: ReactPointerEvent<HTMLElement>) => void;
}) {
  const { t } = useTranslation();
  const requestFileNav = useUIStore((s) => s.requestFileNav);
  const requestArtifactNav = useUIStore((s) => s.requestArtifactNav);
  // One menu at a time for the whole rail: a chip's, or the rail's own.
  const [menu, setMenu] = useState<{ anchor: ContextMenuAnchor; items: ContextMenuItem[] } | null>(
    null,
  );
  const expandedDoc = docs.find((d) => d.id === expandedId) ?? null;

  if (!open) return null;
  const empty = agents.length === 0 && docs.length === 0;
  const wide = expandedDoc != null && cardWidth > 0;

  const tailFor = (d: AuxDoc): AuxCardTail => ({
    isExpanded: d.id === expandedId,
    onToggle: () => onToggleDoc(d.id),
    onClose: () => onCloseDoc(d.id),
    onCloseOthers: () => onCloseOtherDocs(d.id),
    onCloseAll: onCloseAllDocs,
    otherCount: docs.length - 1,
  });

  /** Where a chip's 在…打开 sends this kind. Absent when the destination needs
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

  /** The rail's own menu: what to do with the *stack*, not with one card. */
  const railItems = (): ContextMenuItem[] => {
    const items: ContextMenuItem[] = [];
    if (expandedDoc) {
      items.push({
        id: "collapse",
        label: t("detail.aux_collapse_card", "收起此卡"),
        icon: <ChevronsDownUp size={13} strokeWidth={1.7} />,
        onSelect: onCollapseDoc,
      });
    }
    if (docs.length > 0) {
      items.push({
        id: "close-all",
        label: t("detail.aux_close_all_docs", "关闭全部 {{count}} 张文档卡", {
          count: docs.length,
        }),
        icon: <XCircle size={13} strokeWidth={1.7} />,
        onSelect: onCloseAllDocs,
      });
    }
    items.push({
      id: "hide",
      label: t("detail.rail_hide", "收起辅助栏"),
      icon: <PanelRightClose size={13} strokeWidth={1.7} />,
      dividerBefore: items.length > 0,
      onSelect: onHideRail,
    });
    return items;
  };

  // No drag region on the <aside>: the column is pointer-events:none between
  // the cards so the transcript underneath keeps the wheel and the clicks.
  return (
    <aside
      className={`${styles.rail} ${wide ? styles.rail_wide : ""}`}
      style={wide ? { width: cardWidth } : undefined}
      onContextMenu={(e) => {
        // Reached only by the rail's own background — a chip and an expanded
        // card both stop propagation with their own menu.
        e.preventDefault();
        e.stopPropagation();
        setMenu({ anchor: { x: e.clientX, y: e.clientY }, items: railItems() });
      }}
    >
      {/* Held open by the switch with nothing in it. Saying so beats an empty
          column, which reads as the rail having failed to load rather than as
          "there is genuinely nothing running and nothing opened yet". */}
      {empty && <p className={styles.rail_empty}>{t("detail.rail_empty", "暂无运行中的 Agent 或已打开的文档")}</p>}
      <SubagentLiveCards agents={agents} onOpen={onOpenAgent} />
      {/* Newest first: the file the agent just named is the one you are most
          likely to be reaching for, and it lands nearest the live agents. */}
      {[...docs].reverse().map((d) => {
        const Icon = DOC_ICON[d.kind];
        const isOpen = d.id === expandedId;
        const meta = auxDocMeta(d.kind, d.ref);
        if (!isOpen) {
          return (
            <div
              key={d.id}
              className={`${styles.rail_card} ${styles.doc_card}`}
              onContextMenu={(e) => openChipMenu(d, e)}
            >
              <button
                type="button"
                className={styles.doc_card_main}
                onClick={() => onToggleDoc(d.id)}
                title={d.ref}
                aria-expanded={false}
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
            {/* No title strip above the reader: its own AuxDocBar is the header
                (see the note on this component), and that bar carries both the
                collapse and the ✕. */}
            <div className={styles.aux_doc_pane}>
              <SessionAuxDoc
                doc={d}
                tail={tailFor(d)}
                workspacePath={workspacePath}
                onOpenWiki={onOpenWiki}
              />
            </div>
          </div>
        );
      })}
      {menu && (
        <ContextMenu anchor={menu.anchor} items={menu.items} onClose={() => setMenu(null)} />
      )}
    </aside>
  );
}
