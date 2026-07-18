// IDE-style tab strip above the 启动台 detail column.
//
// Each open session is one tab. The strip owns no session state of its own —
// HistoryView holds the tab list and hands it down, so the tabs and the
// detail column can never disagree about what is open.
//
// The status dot is deliberately resolved from `useSessionsStore` rather than
// from the `SessionInfo` snapshot the tab was opened with: a tab that has been
// sitting in the background for ten minutes must still show whether its agent
// is running *right now*, and it wears the same green/amber dot as the list row
// it came from (`rowBarColor`).

import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useSessionsStore } from "../store";
import { isKeyboardActivationKey } from "../keyboard";
import { preferredSessionTitle, rowBarColor, type SessionInfo } from "../types";
import { ContextMenu, useContextMenu, type ContextMenuItem } from "./ContextMenu";
import styles from "./SessionTabs.module.css";

/** What the tab wears as its name. Mirrors the list row's title, so a session
 *  reads the same in the list and in the strip. Falls back down to a short id
 *  for sessions the scanner hasn't titled yet. */
function tabLabel(s: SessionInfo): string {
  return preferredSessionTitle(s) || s.slug || s.workspaceName || s.id.slice(0, 8);
}

/** One entry in the strip. `session` is the live session behind the tab, or
 *  `null` for the synthetic new-session draft tab, which shows `label` instead
 *  of a title + status dot. */
export interface TabItem {
  id: string;
  session: SessionInfo | null;
  label?: string;
}

export function SessionTabs({
  tabs,
  activeId,
  onActivate,
  onClose,
  onCloseOthers,
  onCloseRight,
  onCloseAll,
  onReorder,
}: {
  tabs: TabItem[];
  activeId: string | null;
  onActivate: (id: string) => void;
  onClose: (id: string) => void;
  onCloseOthers: (id: string) => void;
  onCloseRight: (id: string) => void;
  onCloseAll: () => void;
  /** Move `fromId` to `toId`'s slot. Called live during a drag, not on drop. */
  onReorder: (fromId: string, toId: string) => void;
}) {
  const { t } = useTranslation();
  const sessions = useSessionsStore((s) => s.sessions);
  const menu = useContextMenu();
  // Which tab the open context menu belongs to — `useContextMenu` only tracks
  // the cursor anchor, not the subject.
  const [menuTabId, setMenuTabId] = useState<string | null>(null);
  const [dragId, setDragId] = useState<string | null>(null);
  const activeRef = useRef<HTMLDivElement>(null);

  const liveById = useMemo(
    () => new Map(sessions.map((s) => [s.id, s])),
    [sessions],
  );

  // Activating a tab that is scrolled out of view (e.g. the user clicked its
  // row in the list) should bring it back into the strip.
  useEffect(() => {
    activeRef.current?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [activeId]);

  if (tabs.length === 0) return null;

  const menuItems = (id: string): ContextMenuItem[] => {
    const idx = tabs.findIndex((s) => s.id === id);
    const items: ContextMenuItem[] = [
      { id: "close", label: t("tabs.close"), onSelect: () => onClose(id) },
    ];
    if (tabs.length > 1) {
      items.push({
        id: "close_others",
        label: t("tabs.close_others"),
        onSelect: () => onCloseOthers(id),
      });
    }
    if (idx >= 0 && idx < tabs.length - 1) {
      items.push({
        id: "close_right",
        label: t("tabs.close_right"),
        onSelect: () => onCloseRight(id),
      });
    }
    items.push({
      id: "close_all",
      label: t("tabs.close_all"),
      danger: true,
      onSelect: onCloseAll,
    });
    return items;
  };

  return (
    <div className={styles.strip} role="tablist">
      {tabs.map((tab) => {
        // The draft tab has no backing session — it wears its own label and no
        // status dot. Real tabs resolve their live session from the store so a
        // long-parked tab still shows the agent's current title and dot.
        const live = tab.session ? liveById.get(tab.id) ?? tab.session : null;
        const dot = live ? rowBarColor(live) : null;
        const isActive = tab.id === activeId;
        const label = live ? tabLabel(live) : tab.label ?? "";
        return (
          <div
            key={tab.id}
            ref={isActive ? activeRef : undefined}
            role="tab"
            aria-selected={isActive}
            tabIndex={isActive ? 0 : -1}
            className={`${styles.tab} ${isActive ? styles.tab_active : ""} ${
              tab.id === dragId ? styles.tab_dragging : ""
            }`}
            title={live ? `${label}\n${live.workspaceName}` : label}
            draggable
            onDragStart={(e) => {
              setDragId(tab.id);
              e.dataTransfer.effectAllowed = "move";
            }}
            // Reorder live as the pointer crosses a neighbour rather than on
            // drop, so the strip shows the result under the cursor. Once the
            // swap lands, the tab under the pointer *is* the dragged one, so
            // the id guard below stops this from thrashing on repeat fires.
            onDragOver={(e) => {
              e.preventDefault();
              e.dataTransfer.dropEffect = "move";
              if (!dragId || dragId === tab.id) return;
              onReorder(dragId, tab.id);
            }}
            onDrop={(e) => {
              e.preventDefault();
              setDragId(null);
            }}
            onDragEnd={() => setDragId(null)}
            onClick={() => onActivate(tab.id)}
            onKeyDown={(e) => {
              // The nested close button owns its own keyboard events.
              if (e.target !== e.currentTarget) return;
              if (isKeyboardActivationKey(e.key)) {
                e.preventDefault();
                onActivate(tab.id);
                return;
              }
              let nextIdx: number | null = null;
              const idx = tabs.findIndex((candidate) => candidate.id === tab.id);
              if (e.key === "ArrowRight") nextIdx = (idx + 1) % tabs.length;
              if (e.key === "ArrowLeft") nextIdx = (idx - 1 + tabs.length) % tabs.length;
              if (e.key === "Home") nextIdx = 0;
              if (e.key === "End") nextIdx = tabs.length - 1;
              if (nextIdx == null) return;
              e.preventDefault();
              onActivate(tabs[nextIdx].id);
              const tabEls = e.currentTarget.parentElement?.querySelectorAll<HTMLElement>(
                '[role="tab"]',
              );
              tabEls?.[nextIdx]?.focus();
            }}
            onContextMenu={(e) => {
              setMenuTabId(tab.id);
              menu.open(e);
            }}
            // Middle-click closes, the way it does in every editor. `onAuxClick`
            // rather than `onMouseDown` so a drag that happens to start with the
            // middle button doesn't close the tab out from under the cursor.
            onAuxClick={(e) => {
              if (e.button !== 1) return;
              e.preventDefault();
              onClose(tab.id);
            }}
          >
            {tab.session && (
              <span
                className={styles.dot}
                style={dot ? { background: dot } : undefined}
                data-idle={dot ? undefined : ""}
              />
            )}
            <span className={styles.label}>{label}</span>
            <button
              className={styles.close}
              aria-label={t("tabs.close")}
              // Keep the click off the tab body, which would re-activate it.
              onClick={(e) => {
                e.stopPropagation();
                onClose(tab.id);
              }}
            >
              ✕
            </button>
          </div>
        );
      })}

      {menu.anchor && menuTabId && (
        <ContextMenu
          anchor={menu.anchor}
          items={menuItems(menuTabId)}
          onClose={() => {
            menu.close();
            setMenuTabId(null);
          }}
        />
      )}
    </div>
  );
}
