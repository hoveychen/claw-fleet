import { describe, expect, it } from "vitest";

import { posterTime } from "./mediaThumb";

describe("posterTime", () => {
  /**
   * The decoder hands over whatever the container claims, and two of those
   * values cannot be seeked to: `NaN` when the metadata has not been parsed and
   * `Infinity` for a stream or an mp4 with an unset duration box. Assigning
   * either to `currentTime` leaves the element waiting for a `seeked` that never
   * fires — no error, so the card would sit on its icon with a decoder still
   * running behind it.
   */
  it("falls back to frame 0 for a duration that cannot be seeked to", () => {
    expect(posterTime(Number.NaN)).toBe(0);
    expect(posterTime(Number.POSITIVE_INFINITY)).toBe(0);
    expect(posterTime(0)).toBe(0);
    expect(posterTime(-1)).toBe(0);
  });

  /**
   * Frame 0 is the obvious choice and the worst one: renders open on a fade-in
   * or an empty background, so a grid of first frames is a grid of black
   * rectangles. A tenth in clears that on a short clip…
   */
  it("skips a short clip's fade-in", () => {
    expect(posterTime(4)).toBeCloseTo(0.4);
    expect(posterTime(0.5)).toBeCloseTo(0.05);
  });

  /** …and the cap keeps a long render's poster at its opening, not a minute in. */
  it("caps at one second so a long render still shows its opening", () => {
    expect(posterTime(10)).toBe(1);
    expect(posterTime(7200)).toBe(1);
  });
});
