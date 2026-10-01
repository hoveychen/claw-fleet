import { useEffect, useRef, useState } from "react";

/** Show-delay before a loader appears: waits shorter than this never flash. */
export const LOADER_SHOW_DELAY_MS = 200;
/** Once shown, a loader stays at least this long so it never blinks. */
export const LOADER_MIN_VISIBLE_MS = 300;

/**
 * Debounces a "busy" flag for display. Returns true only after `active` has
 * been true for `delayMs`, and once true stays true for at least `minMs`
 * even if `active` drops sooner.
 */
export function useDelayedFlag(
  active: boolean,
  delayMs: number = LOADER_SHOW_DELAY_MS,
  minMs: number = LOADER_MIN_VISIBLE_MS,
): boolean {
  const [visible, setVisible] = useState(active && delayMs <= 0);
  const shownAt = useRef<number | null>(visible ? Date.now() : null);

  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    if (active) {
      if (shownAt.current === null) {
        timer = setTimeout(() => {
          shownAt.current = Date.now();
          setVisible(true);
        }, Math.max(0, delayMs));
      }
    } else if (shownAt.current !== null) {
      const left = minMs - (Date.now() - shownAt.current);
      const hide = () => {
        shownAt.current = null;
        setVisible(false);
      };
      if (left <= 0) hide();
      else timer = setTimeout(hide, left);
    }
    return () => {
      if (timer) clearTimeout(timer);
    };
  }, [active, delayMs, minMs]);

  return visible;
}
