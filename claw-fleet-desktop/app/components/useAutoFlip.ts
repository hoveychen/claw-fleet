// Auto-flipping popover placement, shared by every absolutely-positioned menu
// that hangs off a composer or pill (PillMenu, the wiki @-mention picker, …).

import { useLayoutEffect, useState, type RefObject } from "react";

export type Side = "above" | "below";

/**
 * Decide which side a menu of `needed` px should open on. Stays on the
 * preferred side unless it doesn't fit there and the opposite side is roomier.
 */
export function pickSide(
  placement: Side,
  roomAbove: number,
  roomBelow: number,
  needed: number,
): Side {
  const preferred = placement === "above" ? roomAbove : roomBelow;
  const opposite = placement === "above" ? roomBelow : roomAbove;
  const flipped = preferred < needed && opposite > preferred;
  return flipped ? (placement === "above" ? "below" : "above") : placement;
}

/**
 * Measure the room around `anchorRef` and return the side the menu should open
 * on. The menu is `position: absolute`, so it's clipped by the nearest ancestor
 * with `overflow-y != visible`; that ancestor (or the viewport) is the bound.
 *
 * `anchorRef` defaults to the menu's `offsetParent` — right for menus that span
 * their container (`left: 0; right: 0`) rather than hanging off a small trigger.
 *
 * `revision` re-measures when the menu's contents change height (e.g. the
 * mention picker's result list shrinking as the query narrows).
 */
export function useAutoFlip(
  open: boolean,
  placement: Side,
  menuRef: RefObject<HTMLElement | null>,
  anchorRef?: RefObject<HTMLElement | null>,
  revision?: unknown,
): Side {
  const [flipped, setFlipped] = useState(false);

  useLayoutEffect(() => {
    if (!open) {
      setFlipped(false);
      return;
    }
    const menu = menuRef.current;
    const anchor = anchorRef?.current ?? (menu?.offsetParent as HTMLElement | null);
    if (!menu || !anchor) return;

    let boundsTop = 0;
    let boundsBottom = window.innerHeight;
    for (let el = anchor.parentElement; el; el = el.parentElement) {
      if (getComputedStyle(el).overflowY !== "visible") {
        const r = el.getBoundingClientRect();
        boundsTop = r.top;
        boundsBottom = r.bottom;
        break;
      }
    }

    const a = anchor.getBoundingClientRect();
    const needed = menu.offsetHeight + 6;
    const side = pickSide(placement, a.top - boundsTop, boundsBottom - a.bottom, needed);
    setFlipped(side !== placement);
  }, [open, placement, menuRef, anchorRef, revision]);

  return flipped ? (placement === "above" ? "below" : "above") : placement;
}
