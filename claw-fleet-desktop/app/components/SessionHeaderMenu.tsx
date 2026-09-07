import { useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import {
  Coins,
  Copy,
  FileClock,
  FileJson2,
  Folder,
  FolderOpen,
  ListChecks,
  type LucideIcon,
  MessageSquareQuote,
  MoreHorizontal,
  Sparkles,
  Terminal,
  Workflow,
} from "lucide-react";
import { ContextMenu, type ContextMenuAnchor, type ContextMenuItem } from "./ContextMenu";
import styles from "./SessionHeaderMenu.module.css";
import { canRevealPath } from "../canReveal";
import type { AuxFacet, AuxFacetItem } from "../detailAux";

/** One icon per facet, so the menu reads as a list of destinations rather than
 *  a wall of text. Keyed by facet id — adding a facet without an icon still
 *  renders, just without one. */
const FACET_ICONS: Record<AuxFacet, LucideIcon> = {
  skills: Sparkles,
  decisions: MessageSquareQuote,
  tokens: Coins,
  tasks: ListChecks,
  bgtasks: Terminal,
  scratchpad: FileClock,
  workflow: Workflow,
};

/**
 * The detail header's overflow menu.
 *
 * Two families live here. First the session's facets — Skills, 决策, Token,
 * 任务, 后台任务, 临时文件, Workflow — each one a button that pulls that panel up
 * in the auxiliary column. They used to be a permanent tab strip above the
 * panel, which spent the strip's whole width on destinations you visit once an
 * hour; as menu items they cost nothing until asked for, and the strip is left
 * to hold only what is actually open.
 *
 * Then the copy-when-you-need-it identifiers — the session id, its transcript
 * path, the workspace path — which were themselves two permanent header rows
 * before they moved in here for the same reason.
 */
export function SessionHeaderMenu({
  sessionId,
  jsonlPath,
  workspacePath,
  facets = [],
  activeFacet = null,
  onPickFacet,
}: {
  sessionId: string;
  jsonlPath: string;
  workspacePath: string;
  /** Facets this session can show, already labelled (counts included). */
  facets?: AuxFacetItem[];
  /** The one the auxiliary panel is showing, if any. */
  activeFacet?: AuxFacet | null;
  onPickFacet?: (id: AuxFacet) => void;
}) {
  const { t } = useTranslation();
  const btnRef = useRef<HTMLButtonElement>(null);
  const [anchor, setAnchor] = useState<ContextMenuAnchor | null>(null);

  // A failed clipboard write (Tauri ACL) must not read as success, but the menu
  // is gone by then — so surface it on the button itself.
  const [failed, setFailed] = useState(false);
  const copy = (text: string) => {
    writeText(text).catch(() => {
      setFailed(true);
      setTimeout(() => setFailed(false), 2000);
    });
  };

  const revealKey =
    document.documentElement.getAttribute("data-platform") === "windows"
      ? "paths.reveal_in_explorer"
      : "paths.reveal_in_finder";

  const items: ContextMenuItem[] = [];

  // Leads the menu: these are the items that *do* something to the layout,
  // where the rest hand you a string to paste elsewhere.
  if (onPickFacet) {
    for (const facet of facets) {
      const Icon = FACET_ICONS[facet.id];
      items.push({
        id: `facet-${facet.id}`,
        label: facet.label,
        icon: Icon ? <Icon size={13} /> : undefined,
        active: activeFacet === facet.id,
        onSelect: () => onPickFacet(facet.id),
      });
    }
  }

  items.push(
    {
      id: "copy-id",
      dividerBefore: items.length > 0,
      label: t("detail.copy_session_id"),
      sub: sessionId,
      icon: <Copy size={13} />,
      onSelect: () => copy(sessionId),
    },
    {
      id: "copy-transcript",
      label: t("detail.copy_transcript_path"),
      sub: jsonlPath,
      icon: <FileJson2 size={13} />,
      onSelect: () => copy(jsonlPath),
    },
    {
      id: "copy-workspace",
      label: t("detail.copy_workspace_path"),
      sub: workspacePath,
      icon: <Folder size={13} />,
      onSelect: () => copy(workspacePath),
    },
  );

  // A browser tab cannot open a file manager; see canReveal.ts.
  if (canRevealPath()) {
    items.push({
      id: "reveal",
      label: t(revealKey),
      icon: <FolderOpen size={13} />,
      onSelect: () => {
        invoke("reveal_path", { path: workspacePath }).catch(() => {
          setFailed(true);
          setTimeout(() => setFailed(false), 2000);
        });
      },
    });
  }

  return (
    <>
      <button
        ref={btnRef}
        type="button"
        className={`${styles.btn} ${anchor ? styles.btn_open : ""} ${failed ? styles.btn_failed : ""}`}
        title={t("detail.more")}
        aria-label={t("detail.more")}
        aria-haspopup="menu"
        aria-expanded={anchor != null}
        onClick={() => {
          if (anchor) {
            setAnchor(null);
            return;
          }
          const r = btnRef.current?.getBoundingClientRect();
          if (!r) return;
          // Hang it off the button, not the cursor. ContextMenu clamps to the
          // viewport, so a button near the right edge flips the menu inward.
          setAnchor({ x: r.left, y: r.bottom + 4 });
        }}
      >
        <MoreHorizontal size={15} />
      </button>
      {anchor && (
        <ContextMenu anchor={anchor} items={items} onClose={() => setAnchor(null)} />
      )}
    </>
  );
}
