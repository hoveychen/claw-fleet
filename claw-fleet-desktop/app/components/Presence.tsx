import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { prefersReducedMotion } from "../hooks/useHeightTransition";

/** How long a closing surface stays mounted to play its exit. Matches the
 *  `*_out` keyframes in motion.module.css; exits run faster than entrances. */
export const EXIT_MS = 140;

const ExitingContext = createContext(false);

/**
 * True while the surrounding <Presence> is playing its exit. A surface that
 * portals out of the Presence wrapper (ContextMenu, the modals rendered into
 * document.body) is not a DOM descendant of the wrapper's `data-exiting`, so it
 * puts the attribute on its own root: `data-exiting={useExiting() || undefined}`.
 * Context, unlike the DOM, does follow a portal.
 */
export function useExiting(): boolean {
  return useContext(ExitingContext);
}

/**
 * Drop-in for `{open && <Dialog …/>}` that lets the dialog animate out. When
 * `when` turns false the last rendered children stay mounted for EXIT_MS under
 * a `data-exiting` wrapper, which the exit rules in motion.module.css key on,
 * then unmount. The children are frozen at their last open render, so a call
 * site like `{target && <Confirm target={target} />}` keeps showing the target
 * it was opened for while it fades, even though `target` is already null.
 *
 * The wrapper is `display: contents` and is present whenever anything is
 * shown — switching between a bare child and a wrapped one would remount the
 * dialog (and wipe its input) at the moment it starts closing.
 *
 * Reopening mid-exit shows the new children at once. Reduced-motion users get
 * the old instant unmount.
 */
export function Presence({ when, children }: { when: boolean; children: ReactNode }) {
  const last = useRef<ReactNode>(null);
  if (when) last.current = children;

  // Derived state, set during render, so the exit starts in the same commit
  // that `when` falls — an effect would unmount for one frame first.
  // The state update only lands on the re-render React schedules right away;
  // this pass still sees the old `exiting`, so it decides from `startExit`.
  const [prevWhen, setPrevWhen] = useState(when);
  const [exiting, setExiting] = useState(false);
  let startExit = false;
  if (prevWhen !== when) {
    startExit = !when && !prefersReducedMotion();
    setPrevWhen(when);
    setExiting(startExit);
  }

  useEffect(() => {
    if (!exiting) return;
    const timer = window.setTimeout(() => {
      last.current = null;
      setExiting(false);
    }, EXIT_MS);
    return () => window.clearTimeout(timer);
  }, [exiting]);

  if (!when && !exiting && !startExit) return null;
  const closing = !when;
  return (
    <ExitingContext.Provider value={closing}>
      <div style={{ display: "contents" }} data-exiting={closing || undefined}>
        {closing ? last.current : children}
      </div>
    </ExitingContext.Provider>
  );
}
