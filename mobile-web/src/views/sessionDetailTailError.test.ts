import { describe, expect, it } from "vitest";
import { shouldShowTailError } from "./SessionDetailView";

describe("shouldShowTailError", () => {
  it("reports the first failure when there is nothing on screen", () => {
    expect(shouldShowTailError(false, 1)).toBe(true);
  });

  it("stays quiet about a single blip once messages are showing", () => {
    // One dropped frame on a mobile link is noise; a banner for it would cry
    // wolf on an otherwise working view.
    expect(shouldShowTailError(true, 1)).toBe(false);
  });

  it("speaks up once failures stop being a blip", () => {
    // This is the case the view used to swallow entirely: after one successful
    // load, every later failure was silent, so a transcript that had stopped
    // following its session looked exactly like a session that went quiet.
    expect(shouldShowTailError(true, 2)).toBe(true);
    expect(shouldShowTailError(true, 9)).toBe(true);
  });
});
