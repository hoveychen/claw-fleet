import { describe, expect, it } from "vitest";
import { wantsSystemBrowser } from "./webLinks";

/** The subset of a React mouse event the decision actually reads. */
function ev(over: Partial<Parameters<typeof wantsSystemBrowser>[0]> = {}) {
  return { button: 0, metaKey: false, ctrlKey: false, altKey: false, ...over };
}

describe("wantsSystemBrowser", () => {
  it("sends a plain click to the in-app tab", () => {
    expect(wantsSystemBrowser(ev())).toBe(false);
  });

  it("sends ⌘-click and ctrl-click out to the browser", () => {
    // The one modifier every browser already means "open this elsewhere" with.
    expect(wantsSystemBrowser(ev({ metaKey: true }))).toBe(true);
    expect(wantsSystemBrowser(ev({ ctrlKey: true }))).toBe(true);
  });

  it("sends a middle click out to the browser", () => {
    // Same idiom as the tab strip, where middle-click means "not here".
    expect(wantsSystemBrowser(ev({ button: 1 }))).toBe(true);
  });

  it("ignores alt, which is a download/other-gesture modifier", () => {
    expect(wantsSystemBrowser(ev({ altKey: true }))).toBe(false);
  });
});
