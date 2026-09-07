import { useCallback, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { ChevronRight, Folder, FolderOpen } from "lucide-react";
import type { SessionInfo } from "../types";
import { SessionRow } from "./SessionRow";
import { GroupMarkControl } from "./MarkControl";
import {
  chainBarColor,
  GROUP_LOAD_STEP,
  GROUP_VISIBLE,
  type RenderItem,
} from "./sessionGroups";
import styles from "./SessionRail.module.css";

type SessionRailProps = {
  /** Pre-folded render items (singles + collapsed relay groups). Compute with
   *  `buildRenderItems(rows, group)` in the parent so each caller controls its
   *  own filtering/sorting/freeze before folding. */
  items: RenderItem[];
  /** Full membership of every relay chain keyed by chainId, taken over ALL
   *  launchpad sessions (not just the filtered rows) so a group header's
   *  aggregate liveness / mark-all covers the whole chain even when a
   *  filter hides some hops. */
  chainMembersAll: Map<string, SessionInfo[]>;
  /** Session currently shown in the detail column (rendered as .row_active). */
  activeId: string | null;
  /** Sessions open in a background tab but not on screen (.row_open). Pass an
   *  empty set where there are no tabs. */
  openIds: Set<string>;
  /** FTS snippet for a row's transcript, or undefined when the query is too
   *  short / didn't match. Parent owns the query threshold. */
  snippetFor: (jsonlPath: string) => string | undefined;
  /** Bumped every 30s by the parent so relative times keep advancing. */
  nowTick: number;
  /** Show the agent-source glyph ahead of each title (only when sources mix). */
  showSource: boolean;
  /** Hidden when a repository heading already labels these rows. */
  showWorkspace?: boolean;
  onRowClick: (s: SessionInfo) => void;
  onContextMenu: (e: React.MouseEvent, s: SessionInfo) => void;
};

type WorkspaceRailSectionProps = {
  path: string;
  name: string;
  /** 折叠后的组数（一条折叠的接力链算一组），不是会话总数。 */
  count: number;
  children: ReactNode;
};

/** A repository heading around one SessionRail. Directory grouping is kept
 * outside SessionRail itself so the relay-chain renderer stays unchanged and
 * can still be reused as a flat list. */
export function WorkspaceRailSection({
  path,
  name,
  count,
  children,
}: WorkspaceRailSectionProps) {
  const { t } = useTranslation();
  const [collapsed, setCollapsed] = useState(false);

  return (
    <section className={styles.workspace_section}>
      <button
        type="button"
        className={styles.workspace_header}
        aria-expanded={!collapsed}
        title={path}
        onClick={() => setCollapsed((value) => !value)}
      >
        <span className={styles.workspace_folder} aria-hidden="true">
          {collapsed ? <Folder size={14} /> : <FolderOpen size={14} />}
        </span>
        <span className={styles.workspace_name}>{name}</span>
        <span
          className={styles.workspace_count}
          aria-label={t("history.workspace_session_count", "{{count}} 组会话", { count })}
        >
          {count}
        </span>
        <ChevronRight
          className={styles.workspace_chevron}
          size={13}
          data-open={!collapsed}
          aria-hidden="true"
        />
      </button>
      {!collapsed && <div className={styles.workspace_sessions}>{children}</div>}
    </section>
  );
}

/**
 * The grouped session list — the "二级侧边栏" rail shared by the desktop task
 * page (HistoryView). Renders standalone rows and
 * collapsed handoff-relay chains (a tip header that expands to show earlier
 * hops), owning only the local expand / page-in state; everything data-shaped
 * (which sessions, their order, snippets, active state) is supplied by the
 * parent so both callers render the identical rail.
 */
export function SessionRail({
  items,
  chainMembersAll,
  activeId,
  openIds,
  snippetFor,
  nowTick,
  showSource,
  showWorkspace = true,
  onRowClick,
  onContextMenu,
}: SessionRailProps) {
  const { t } = useTranslation();
  // Which chains are expanded, and how many hops each has paged in beyond the
  // initial GROUP_VISIBLE. Local: default-collapsed is the right reset on remount.
  const [expandedChains, setExpandedChains] = useState<Set<string>>(() => new Set());
  const [chainLoadMore, setChainLoadMore] = useState<Record<string, number>>({});
  const toggleChain = useCallback((cid: string) => {
    setExpandedChains((prev) => {
      const next = new Set(prev);
      if (next.has(cid)) next.delete(cid);
      else next.add(cid);
      return next;
    });
  }, []);
  const loadMoreChain = useCallback((cid: string) => {
    setChainLoadMore((prev) => ({
      ...prev,
      [cid]: (prev[cid] ?? GROUP_VISIBLE) + GROUP_LOAD_STEP,
    }));
  }, []);

  const renderRow = (s: SessionInfo) => (
    <SessionRow
      key={s.jsonlPath}
      session={s}
      snippet={snippetFor(s.jsonlPath)}
      isSelected={activeId === s.id}
      isOpen={activeId !== s.id && openIds.has(s.id)}
      nowTick={nowTick}
      showSource={showSource}
      showWorkspace={showWorkspace}
      onClick={onRowClick}
      onContextMenu={onContextMenu}
    />
  );

  return (
    <>
      {items.map((item) => {
        if (item.kind === "single") return renderRow(item.session);
        const { chainId, tip, members, key } = item;
        // Full chain for the mark-all + aggregate state; falls back to the
        // present members if the scan hasn't indexed the chain yet.
        const full = chainMembersAll.get(chainId) ?? members;
        const expanded = expandedChains.has(chainId);
        const limit = chainLoadMore[chainId] ?? GROUP_VISIBLE;
        // The header *is* the tip session (latest hop), so the expanded list
        // shows only the chain's *other* hops — never the tip again.
        const rest = members.filter((m) => m.id !== tip.id);
        const shown = expanded ? rest.slice(0, limit) : [];
        const hidden = rest.length - shown.length;
        return (
          <div key={key} className={styles.group_wrap}>
            {/* Header = the tip rendered as an ordinary clickable row (opens its
                detail), plus a chevron that reveals the earlier hops and a
                whole-chain mark toggle. */}
            <SessionRow
              session={tip}
              snippet={snippetFor(tip.jsonlPath)}
              isSelected={activeId === tip.id}
              isOpen={activeId !== tip.id && openIds.has(tip.id)}
              // The run-status dot represents the whole collapsed chain, not
              // just the tip: the group is sorted to the top by its
              // most-recently-active member, so its header must show that
              // member's live state or it reads as stale. `full` is the chain's
              // complete membership.
              runColorOverride={chainBarColor(full)}
              nowTick={nowTick}
              showSource={showSource}
              showWorkspace={showWorkspace}
              onClick={onRowClick}
              onContextMenu={onContextMenu}
              expandable={{
                expanded,
                onToggle: () => toggleChain(chainId),
                markControl: <GroupMarkControl members={full} />,
              }}
            />
            {expanded && (
              <div className={styles.group_children}>
                {shown.map((m) => renderRow(m))}
                {hidden > 0 && (
                  <button
                    type="button"
                    className={styles.group_more}
                    onClick={() => loadMoreChain(chainId)}
                  >
                    {t("history.group_load_more", "显示更早的 {{n}} 棒", { n: hidden })}
                  </button>
                )}
              </div>
            )}
          </div>
        );
      })}
    </>
  );
}
