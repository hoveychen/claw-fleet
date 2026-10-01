// PillMenu — the ghost-pill + popover-menu primitive of the composer design
// language. Extracted so every composer surface renders the exact same control
// instead of hand-rolled copies: a quiet transparent pill that opens a custom
// check-list popover, no native <select> chrome, no form labels.

import { type ReactNode, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, ChevronDown } from "lucide-react";
import { useAutoFlip } from "./useAutoFlip";
import { Spinner } from "./loading";
import { useDelayedFlag } from "../hooks/useDelayedFlag";
import styles from "./PillMenu.module.css";
import { Presence } from "./Presence";

export interface PillMenuItem {
  id: string;
  label: string;
  /** Small mono second line under the label (e.g. a workspace path). */
  sub?: string;
  /** Icon shown in the check column (for action items like "Browse…"). */
  icon?: ReactNode;
  checked?: boolean;
  /** Leave the popover open after selecting. For rows that *navigate* the menu
   *  rather than commit a value — the dsh model picker's vendor folders and its
   *  back row — where closing would make the second level unreachable in one
   *  gesture. Committing rows leave this unset and close as before. */
  keepOpen?: boolean;
  onSelect: () => void | Promise<void>;
}

export interface PillMenuProps {
  /** Pill text (ellipsized past max-width). */
  label: string;
  /** Rendered in place of `label` when set — e.g. a skeleton while the value
   *  the pill would name is still being worked out. */
  labelSlot?: ReactNode;
  /** Optional leading icon on the pill (e.g. a folder for the workspace pill). */
  icon?: ReactNode;
  title?: string;
  disabled?: boolean;
  /**
   * Preferred side of the pill the popover opens on. If the nearest clipping
   * ancestor (first `overflow != visible`, e.g. SessionDetail's
   * overflow:hidden root) leaves too little room on that side and the other
   * side has more, the menu flips automatically.
   */
  placement: "above" | "below";
  items: PillMenuItem[];
  /** Items rendered after a separator (e.g. "Browse folder…"). */
  footerItems?: PillMenuItem[];
  /**
   * Free-form row pinned above the items (e.g. a manual path input). Receives
   * a close() callback so the row can dismiss the menu after committing.
   */
  menuHeader?: (close: () => void) => ReactNode;
  className?: string;
  /** Stable hook for UI automation (the live-data browser harness drives these
   *  pills; matching on their label breaks the moment the label localizes or
   *  the selected value changes). */
  testId?: string;
  /** Called when the popover is about to open (not on close). Hosts use it to
   *  refetch menu data: the dsh model catalogue is fetched over IPC and its
   *  first load can fail during `dsh web` startup, so reopening must retry or
   *  the menu lies ("no options") until the whole dialog is remounted. */
  onOpen?: () => void;
  /** The menu's options are still being fetched. The pill's chevron turns into
   *  a spinner and the open menu ends in a "loading" row, so a half-filled list
   *  (often just "default") does not read as the complete set. */
  loading?: boolean;
  /** Text of that loading row; defaults to the generic "Loading…". */
  loadingLabel?: string;
}

export function PillMenu({
  label,
  labelSlot,
  icon,
  title,
  disabled,
  placement,
  items,
  footerItems,
  menuHeader,
  className,
  testId,
  onOpen,
  loading,
  loadingLabel,
}: PillMenuProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  // An item's async `onSelect` still settling (a native dialog, a backend
  // write). The menu is already closed by then, so the pill carries the cue —
  // gated so the common synchronous pick never flashes it.
  const [busy, setBusy] = useState(false);
  const showBusy = useDelayedFlag(busy || !!loading);
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const menuRef = useRef<HTMLDivElement | null>(null);

  // Close on outside click.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (!wrapRef.current?.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [open]);

  const side = useAutoFlip(open, placement, menuRef, wrapRef);

  const renderItem = (item: PillMenuItem) => (
    <button
      key={item.id}
      type="button"
      role="menuitem"
      className={styles.menu_item}
      onClick={async () => {
        if (!item.keepOpen) setOpen(false);
        const result = item.onSelect();
        if (!result || typeof (result as Promise<void>).then !== "function") return;
        setBusy(true);
        try {
          await result;
        } finally {
          setBusy(false);
        }
      }}
    >
      {item.icon ?? (
        <Check
          size={13}
          strokeWidth={2.2}
          className={item.checked ? styles.check_on : styles.check_off}
        />
      )}
      {item.sub ? (
        <span className={styles.menu_item_text}>
          <span className={styles.menu_item_label}>{item.label}</span>
          <span className={styles.menu_item_sub} title={item.sub}>
            {item.sub}
          </span>
        </span>
      ) : (
        <span className={styles.menu_item_label}>{item.label}</span>
      )}
    </button>
  );

  return (
    <div className={`${styles.menu_wrap} ${className ?? ""}`} ref={wrapRef}>
      <button
        type="button"
        className={styles.ghost_pill}
        onClick={() => {
          if (disabled || busy) return;
          if (!open) onOpen?.();
          setOpen((v) => !v);
        }}
        disabled={disabled}
        title={title}
        data-testid={testId}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-busy={busy || loading || undefined}
      >
        {icon}
        <span className={styles.pill_label}>{labelSlot ?? label}</span>
        {showBusy ? (
          <Spinner size={12} className={styles.pill_chevron} />
        ) : (
          <ChevronDown size={13} strokeWidth={1.8} className={styles.pill_chevron} />
        )}
      </button>
      <Presence when={Boolean(open)}>{open && (
        <div
          ref={menuRef}
          className={`${styles.menu} ${side === "above" ? styles.menu_above : styles.menu_below}`}
          role="menu"
        >
          {menuHeader?.(() => setOpen(false))}
          {items.map(renderItem)}
          {loading && (
            <div className={styles.menu_loading} role="status">
              <Spinner size={12} />
              <span>{loadingLabel ?? t("loading", "Loading…")}</span>
            </div>
          )}
          {footerItems && footerItems.length > 0 && (
            <>
              {(items.length > 0 || menuHeader) && <div className={styles.menu_sep} />}
              {footerItems.map(renderItem)}
            </>
          )}
        </div>
      )}</Presence>
    </div>
  );
}
