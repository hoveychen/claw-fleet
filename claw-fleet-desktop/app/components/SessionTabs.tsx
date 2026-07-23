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
//
// When the tabs overflow the strip they scroll horizontally (no visible bar),
// which makes far-off tabs hard to reach. A ▾ overflow button appears at the
// right end *only while there is overflow* and opens a searchable dropdown of
// every open tab — the VS Code "show opened editors" idiom — so no tab is ever
// stranded off-screen.

import {
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { ChevronDown } from "lucide-react";
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

/** A resolved row for the overflow dropdown — title + live dot, decoupled from
 *  the strip's DOM so the menu can be filtered and keyboard-driven on its own. */
interface OverflowRow {
  id: string;
  label: string;
  /** Dot colour, or `null` for an idle/ended session (muted ring). */
  dot: string | null;
  hasSession: boolean;
  isActive: boolean;
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
  const stripRef = useRef<HTMLDivElement>(null);
  const overflowBtnRef = useRef<HTMLButtonElement>(null);
  const [overflowing, setOverflowing] = useState(false);
  const [overflowOpen, setOverflowOpen] = useState(false);

  const liveById = useMemo(
    () => new Map(sessions.map((s) => [s.id, s])),
    [sessions],
  );

  // Activating a tab that is scrolled out of view (e.g. the user clicked its
  // row in the list) should bring it back into the strip.
  useEffect(() => {
    activeRef.current?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [activeId]);

  // Does the strip overflow? The ▾ button only earns its place when tabs are
  // actually hidden. Remeasured when the strip resizes (window/sidebar) and
  // whenever the tab set changes.
  useEffect(() => {
    const el = stripRef.current;
    if (!el) return;
    const measure = () => setOverflowing(el.scrollWidth > el.clientWidth + 1);
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [tabs.length]);

  // If the overflow disappears (tab closed, window widened) while the dropdown
  // is open, close it — the anchor button is gone.
  useEffect(() => {
    if (!overflowing) setOverflowOpen(false);
  }, [overflowing]);

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

  // The dropdown lists *every* open tab (a quick-switcher), not just the hidden
  // ones — the same resolution as the strip so titles and dots match exactly.
  const overflowRows: OverflowRow[] = tabs.map((tab) => {
    const live = tab.session ? liveById.get(tab.id) ?? tab.session : null;
    return {
      id: tab.id,
      label: live ? tabLabel(live) : tab.label ?? "",
      dot: live ? rowBarColor(live) : null,
      hasSession: !!tab.session,
      isActive: tab.id === activeId,
    };
  });

  return (
    <div className={styles.bar}>
      <div className={styles.strip} role="tablist" ref={stripRef}>
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
      </div>

      {overflowing && (
        <button
          ref={overflowBtnRef}
          className={`${styles.overflow_btn} ${
            overflowOpen ? styles.overflow_btn_active : ""
          }`}
          aria-label={t("tabs.overflow")}
          title={t("tabs.overflow")}
          aria-expanded={overflowOpen}
          onClick={() => setOverflowOpen((v) => !v)}
        >
          <ChevronDown size={15} />
        </button>
      )}

      {overflowOpen && overflowBtnRef.current && (
        <TabOverflowMenu
          anchorEl={overflowBtnRef.current}
          rows={overflowRows}
          placeholder={t("tabs.search")}
          emptyLabel={t("tabs.no_match")}
          onSelect={(id) => {
            onActivate(id);
            setOverflowOpen(false);
          }}
          onClose={() => setOverflowOpen(false)}
        />
      )}

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

/**
 * The overflow dropdown: a searchable list of every open tab, anchored under
 * the ▾ button. Portalled (like `ContextMenu`) so it escapes the strip's
 * `overflow` clip, closes on outside-click / blur / resize / Escape, and is
 * arrow-key navigable from the always-focused search box.
 */
function TabOverflowMenu({
  anchorEl,
  rows,
  placeholder,
  emptyLabel,
  onSelect,
  onClose,
}: {
  anchorEl: HTMLElement;
  rows: OverflowRow[];
  placeholder: string;
  emptyLabel: string;
  onSelect: (id: string) => void;
  onClose: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const [query, setQuery] = useState("");
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  const [hi, setHi] = useState(0);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return rows;
    return rows.filter((r) => r.label.toLowerCase().includes(q));
  }, [rows, query]);

  // Reset the highlight to the top whenever the filter narrows the list.
  useEffect(() => {
    setHi(0);
  }, [query]);

  // Anchor under the button's right edge, clamped into the viewport. Measured
  // before paint so it never flashes off-screen.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const btn = anchorEl.getBoundingClientRect();
    const { width, height } = el.getBoundingClientRect();
    const PAD = 4;
    let left = btn.right - width;
    if (left + width > window.innerWidth - PAD) left = window.innerWidth - width - PAD;
    if (left < PAD) left = PAD;
    let top = btn.bottom + 2;
    if (top + height > window.innerHeight - PAD) {
      top = Math.max(PAD, btn.top - height - 2);
    }
    setPos({ left, top });
  }, [anchorEl, filtered.length]);

  useEffect(() => {
    inputRef.current?.focus();
    const onDown = (e: MouseEvent) => {
      const target = e.target as Node;
      // Clicks on the anchor button itself fall through to its own toggle so we
      // don't close-then-reopen.
      if (ref.current?.contains(target) || anchorEl.contains(target)) return;
      onClose();
    };
    document.addEventListener("mousedown", onDown, true);
    window.addEventListener("blur", onClose);
    window.addEventListener("resize", onClose);
    return () => {
      document.removeEventListener("mousedown", onDown, true);
      window.removeEventListener("blur", onClose);
      window.removeEventListener("resize", onClose);
    };
  }, [onClose, anchorEl]);

  return createPortal(
    <div
      ref={ref}
      className={styles.overflow_menu}
      style={{
        left: pos?.left ?? -9999,
        top: pos?.top ?? -9999,
        visibility: pos ? "visible" : "hidden",
      }}
    >
      <input
        ref={inputRef}
        className={styles.overflow_search}
        value={query}
        placeholder={placeholder}
        onChange={(e) => setQuery(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.preventDefault();
            onClose();
          } else if (e.key === "ArrowDown") {
            e.preventDefault();
            setHi((i) => Math.min(i + 1, filtered.length - 1));
          } else if (e.key === "ArrowUp") {
            e.preventDefault();
            setHi((i) => Math.max(i - 1, 0));
          } else if (e.key === "Enter") {
            e.preventDefault();
            const row = filtered[hi];
            if (row) onSelect(row.id);
          }
        }}
      />
      <div className={styles.overflow_list}>
        {filtered.length === 0 ? (
          <div className={styles.overflow_empty}>{emptyLabel}</div>
        ) : (
          filtered.map((row, i) => (
            <button
              key={row.id}
              className={`${styles.overflow_item} ${
                row.isActive ? styles.overflow_item_active : ""
              } ${i === hi ? styles.overflow_item_hi : ""}`}
              onMouseEnter={() => setHi(i)}
              onClick={() => onSelect(row.id)}
            >
              <span
                className={styles.dot}
                style={row.hasSession && row.dot ? { background: row.dot } : undefined}
                data-idle={row.hasSession && row.dot ? undefined : ""}
              />
              <span className={styles.overflow_item_label}>{row.label}</span>
            </button>
          ))
        )}
      </div>
    </div>,
    document.body,
  );
}
