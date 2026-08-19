// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { reviveScrollLayer } from "./scrollLayerRevive";

describe("reviveScrollLayer", () => {
  let el: HTMLDivElement;
  /** Every `overflow-y` value the element passed through, in order. */
  let seen: string[];

  beforeEach(() => {
    el = document.createElement("div");
    el.style.overflowY = "auto";
    document.body.appendChild(el);

    seen = [];
    // jsdom never lays anything out, so the only observable effect of the
    // revive is the style it drives the element through. Record every write,
    // passing it through to the real accessor so the element still behaves.
    // jsdom puts these on the style object's immediate prototype
    // (CSSStyleProperties), not on CSSStyleDeclaration.prototype.
    const style = el.style;
    const real = Object.getOwnPropertyDescriptor(
      Object.getPrototypeOf(style),
      "overflowY",
    )!;
    Object.defineProperty(style, "overflowY", {
      configurable: true,
      get(): string {
        return real.get!.call(this) as string;
      },
      set(v: string) {
        seen.push(v);
        real.set!.call(this, v);
      },
    });
  });

  afterEach(() => {
    el.remove();
  });

  it("drives the container out of and back into its scrolling mode", () => {
    // Toggling overflow is what makes WebKit recompute the scrollable extent;
    // reading a layout property in between is what makes it happen *now*
    // rather than at the next paint.
    reviveScrollLayer(el);

    expect(seen).toContain("hidden");
    expect(seen[seen.length - 1]).not.toBe("hidden");
  });

  it("puts the reader back exactly where they were", () => {
    // Dropping to overflow:hidden clamps scrollTop to 0. Losing the reader's
    // place would be a worse bug than the freeze this is fixing.
    Object.defineProperty(el, "scrollHeight", { value: 9000, configurable: true });
    Object.defineProperty(el, "clientHeight", { value: 500, configurable: true });
    el.scrollTop = 4200;

    reviveScrollLayer(el);

    expect(el.scrollTop).toBe(4200);
  });

  it("restores the author's overflow value rather than hard-coding one", () => {
    el.style.overflowY = "scroll";
    seen = [];

    reviveScrollLayer(el);

    expect(el.style.overflowY).toBe("scroll");
  });

  it("reports that it ran", () => {
    expect(reviveScrollLayer(el)).toBe(true);
  });

  it("refuses to re-enter, so a revive can't trigger its own observers", () => {
    // The prevention layer calls this from a ResizeObserver on the very
    // element being restyled. Without a guard, a revive that perturbs layout
    // re-enters through that observer and spins.
    let inner: boolean | null = null;
    const style = el.style;
    const outer = Object.getOwnPropertyDescriptor(style, "overflowY")!;
    Object.defineProperty(style, "overflowY", {
      configurable: true,
      get: outer.get,
      set(v: string) {
        outer.set!.call(this, v);
        if (inner === null) inner = reviveScrollLayer(el);
      },
    });

    reviveScrollLayer(el);

    expect(inner).toBe(false);
  });
});
