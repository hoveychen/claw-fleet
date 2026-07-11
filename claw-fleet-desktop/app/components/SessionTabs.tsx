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
import { rowBarColor, type SessionInfo } from "../types";
import { ContextMenu, useContextMenu, type ContextMenuItem } from "./ContextMenu";
import styles from "./SessionTabs.module.css";

/** What the tab wears as its name. Mirrors the list row's title, so a session
 *  reads the same in the list and in the strip. Falls back down to a short id
 *  for sessions the scanner hasn't titled yet. */
function tabLabel(s: SessionInfo): string {
  return s.aiTitle || s.slug || s.workspaceName || s.id.slice(0, 8);
}

export function SessionTabs({
  tabs,
  activeId,
  onActivate,
  onClose,
  onCloseOthers,
  onCloseRight,
  onCloseAll,
}: {
  tabs: SessionInfo[];
  activeId: string | null;
  onActivate: (id: string) => void;
  onClose: (id: string) => void;
  onCloseOthers: (id: string) => void;
  onCloseRight: (id: string) => void;
  onCloseAll: () => void;
}) {
  const { t } = useTranslation();
  const sessions = useSessionsStore((s) => s.sessions);
  const menu = useContextMenu();
  // Which tab the open context menu belongs to — `useContextMenu` only tracks
  // the cursor anchor, not the subject.
  const [menuTabId, setMenuTabId] = useState<string | null>(null);
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
        const live = liveById.get(tab.id) ?? tab;
        const dot = rowBarColor(live);
        const isActive = tab.id === activeId;
        const label = tabLabel(live);
        return (
          <div
            key={tab.id}
            ref={isActive ? activeRef : undefined}
            role="tab"
            aria-selected={isActive}
            className={`${styles.tab} ${isActive ? styles.tab_active : ""}`}
            title={`${label}\n${live.workspaceName}`}
            onClick={() => onActivate(tab.id)}
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
            <span
              className={styles.dot}
              style={dot ? { background: dot } : undefined}
              data-idle={dot ? undefined : ""}
            />
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
