import { useEffect, useRef } from "react";
import { NavStack, type RootBackResult } from "./navStack";

/** React thin wrapper over NavStack.
 *
 *  The stack must be a module-level lazy singleton, not attached to App's effect: React effects run children before parents,
 *  so when the overlay (child) registers its layer, App's (parent) effect hasn't run yet. */

let stack: NavStack | undefined;
/** Back handling at the stack bottom belongs to App (it knows the current tab and "press again to exit" state). */
let rootBackHandler: () => RootBackResult = () => "leave";

function getStack(): NavStack | undefined {
  if (typeof window === "undefined") return undefined; // Unit tests import this without crashing
  if (!stack) {
    stack = new NavStack(window.history, () => rootBackHandler());
    stack.start();
    // Must listen for the entire app lifecycle, never unbind.
    window.addEventListener("popstate", () => stack?.handlePopState());
  }
  return stack;
}

export function setRootBackHandler(fn: () => RootBackResult): void {
  rootBackHandler = fn;
  getStack(); // Ensure the sentinel is pushed while we're here
}

/** Mount = open a layer, unmount = close a layer. When the user presses back, `onBack` is called to change
 *  React state and close the overlay; conversely, clicking a back button in the page directly changes state, and on unmount
 *  this hook cleans up the corresponding history entry — both directions converge on the same accounting. */
export function useHistoryLayer(onBack: () => void): void {
  const ref = useRef(onBack);
  ref.current = onBack;
  useEffect(() => {
    const s = getStack();
    if (!s) return;
    const id = s.push(() => ref.current());
    return () => s.drop(id);
  }, []);
}

/** For layers without a dedicated component (like "not on the home tab"): rendering it conditionally is how you register it as a layer. */
export function HistoryLayer({ onBack }: { onBack: () => void }): null {
  useHistoryLayer(onBack);
  return null;
}
