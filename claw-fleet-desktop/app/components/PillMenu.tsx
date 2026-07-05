// PillMenu — the ghost-pill + popover-menu primitive of the composer design
// language (see TaskComposer's header comment). Extracted so TaskComposer,
// NewSessionModal and SessionLauncher render the exact same control instead of
// three hand-rolled copies: a quiet transparent pill that opens a custom
// check-list popover, no native <select> chrome, no form labels.

import { type ReactNode, useEffect, useRef, useState } from "react";
import { Check, ChevronDown } from "lucide-react";
import styles from "./PillMenu.module.css";

export interface PillMenuItem {
  id: string;
  label: string;
  /** Small mono second line under the label (e.g. a workspace path). */
  sub?: string;
  /** Icon shown in the check column (for action items like "Browse…"). */
  icon?: ReactNode;
  checked?: boolean;
  onSelect: () => void | Promise<void>;
}

export interface PillMenuProps {
  /** Pill text (ellipsized past max-width). */
  label: string;
  /** Optional leading icon on the pill (e.g. a folder for the workspace pill). */
  icon?: ReactNode;
  title?: string;
  disabled?: boolean;
  /** Which side of the pill the popover opens on. */
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
}

export function PillMenu({
  label,
  icon,
  title,
  disabled,
  placement,
  items,
  footerItems,
  menuHeader,
  className,
}: PillMenuProps) {
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement | null>(null);

  // Close on outside click.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (!wrapRef.current?.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [open]);

  const renderItem = (item: PillMenuItem) => (
    <button
      key={item.id}
      type="button"
      role="menuitem"
      className={styles.menu_item}
      onClick={async () => {
        setOpen(false);
        await item.onSelect();
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
        onClick={() => !disabled && setOpen((v) => !v)}
        disabled={disabled}
        title={title}
        aria-haspopup="menu"
        aria-expanded={open}
      >
        {icon}
        <span className={styles.pill_label}>{label}</span>
        <ChevronDown size={13} strokeWidth={1.8} className={styles.pill_chevron} />
      </button>
      {open && (
        <div
          className={`${styles.menu} ${placement === "above" ? styles.menu_above : styles.menu_below}`}
          role="menu"
        >
          {menuHeader?.(() => setOpen(false))}
          {items.map(renderItem)}
          {footerItems && footerItems.length > 0 && (
            <>
              {(items.length > 0 || menuHeader) && <div className={styles.menu_sep} />}
              {footerItems.map(renderItem)}
            </>
          )}
        </div>
      )}
    </div>
  );
}
