// Icon action button in the top right of detail pages — one place.
//
// `AppHeader` condensed the header skeleton into a component, but its `actions` slot
// is a bare `ReactNode` — so every page that needs a "refresh" button hand-rolled its
// own. Before this, four variations counted separately:
//
//   Page          Glyph                      Color                  Padding
//   Wiki          <RefreshCw size={16}/>    --color-text-secondary 2px 6px (plus legacy
//                                                                   margin-left:auto)
//   Repo          "⟳" text char              --color-text-secondary 2px 6px
//   Plans         "⟳" text char              --color-text-dim       4px 6px
//   Usage         "⟳" text char              --color-text-secondary 2px 6px (only one
//                                                                   with :disabled)
//
// The `⟳` (U+27F3) is the critical bit: it's **a text character**, shaped by the system
// font, not matching the lucide outline chevron on the left of the same header — stroke
// weight and size don't align. Three pages used it, one used an icon, and that's a
// concrete source of "looks like a webpage, not an app".
//
// Also delivers what none of the four had: hit target (32px visible + 44px invisible,
// mirroring .backButton) and spin while busy — without feedback during a few hundred
// milliseconds of network round-trip, the phone user thinks "this button is broken".
//
// Deliberately not `AppHeader`'s `onRefresh` prop: actions are more than just refresh
// (wiki doc pages also export and version-select), turning each into props would replay
// AppHeader's comment about "every escape hatch reopens a new drift". This is one
// **button's** shared implementation, not an abstract action menu.

import type { ReactNode } from "react";
import styles from "./HeaderAction.module.css";

export function HeaderAction({
  icon,
  label,
  onClick,
  busy,
  disabled,
}: {
  /** A lucide icon node. Use an icon, not a text character: text glyphs change with the
   *  system font and don't match the outline chevron on the left of the header. */
  icon: ReactNode;
  /** Accessibility name (button has no visible text). */
  label: string;
  onClick: () => void;
  /** Currently running — the icon spins. Doesn't auto-gray: the spinning already says
   *  "busy", and graying again would make "busy" and "disabled" look identical. */
  busy?: boolean;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      className={styles.action}
      onClick={onClick}
      aria-label={label}
      aria-busy={busy || undefined}
      data-busy={busy ? "true" : undefined}
      disabled={disabled}
    >
      {icon}
    </button>
  );
}
