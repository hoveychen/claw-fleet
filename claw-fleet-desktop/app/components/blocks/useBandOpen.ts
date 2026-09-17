import { useEffect, useState } from "react";

/**
 * Open/closed state of a collapsible transcript fold (`WorkRunBlock`'s work
 * band, `MetaFoldBlock`'s system-context divider).
 *
 * The signals driving these folds are *momentary*. `WorkRunBlock`'s
 * `defaultOpen` is "this band is the last render unit", and it flips several
 * times inside a single turn — a band drops out of last place the moment the
 * agent writes one prose record (prose is not a work row, so it becomes its own
 * unit). `forceOpen` likewise flips as search navigation steps on and off a hit.
 *
 * Mirroring those signals both ways made a live band flap open and closed while
 * the reader was mid-sentence, and stomped a manual toggle on every flip. So
 * the signal is a *latch*: it opens the fold and never closes it. Once open —
 * by the live tail or by the active search hit — only a click closes it again.
 */
export function useBandOpen(defaultOpen: boolean, forceOpen: boolean) {
  const [open, setOpen] = useState(defaultOpen || forceOpen);
  useEffect(() => {
    if (defaultOpen || forceOpen) setOpen(true);
  }, [defaultOpen, forceOpen]);
  return [open, setOpen] as const;
}
