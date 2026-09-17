import type { RootBackResult } from "./navStack";

/** Root-level back's "press again to exit" gate.
 *
 *  Why not rely only on beforeunload: on iOS standalone PWA (opened from home screen),
 *  the browser basically doesn't show that native dialog, and this is precisely Fleet's
 *  mobile's primary form — just hooking beforeunload leaves the most common scenario
 *  unprotected. So what actually prevents accidental taps is here: the first back only
 *  shows a prompt and pushes the sentinel back; press again within 2 seconds to proceed. */

export const EXIT_WINDOW_MS = 2_000;

export class ExitGuard {
  private armed = false;
  private timer: ReturnType<typeof setTimeout> | undefined;

  constructor(
    /** Controls toast visibility. */
    private onArmedChange: (armed: boolean) => void,
    /** Cleanup before allowing: remove beforeunload — the user has already confirmed
     *  their intent once via toast; another native "Leave this website?" is a second
     *  confirmation, pure torture. */
    private onLeave: () => void,
    private windowMs: number = EXIT_WINDOW_MS,
  ) {}

  handleRootBack = (): RootBackResult => {
    if (this.armed) {
      this.disarm();
      this.onLeave();
      return "leave";
    }
    this.armed = true;
    this.onArmedChange(true);
    this.timer = setTimeout(() => this.disarm(), this.windowMs);
    return "hold";
  };

  private disarm(): void {
    if (this.timer !== undefined) clearTimeout(this.timer);
    this.timer = undefined;
    if (!this.armed) return;
    this.armed = false;
    this.onArmedChange(false);
  }
}

/** Fallback confirmation for non-back paths like refresh / close tab / address bar
 *  navigation (text is decided by browser, not customizable). Returns an uninstall function. */
export function installUnloadPrompt(): () => void {
  const onBeforeUnload = (e: BeforeUnloadEvent) => {
    e.preventDefault();
    e.returnValue = "";
  };
  window.addEventListener("beforeunload", onBeforeUnload);
  return () => window.removeEventListener("beforeunload", onBeforeUnload);
}
