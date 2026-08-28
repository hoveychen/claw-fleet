// Desktop cases. The browser-build ones need markWebBuild(), which cannot be
// undone within a module graph — they live in canReveal.web.test.ts.
import { describe, it, expect } from "vitest";
import { canRevealPath } from "./canReveal";

describe("canRevealPath — desktop", () => {
  it("allows a local workspace", () => {
    expect(canRevealPath(true)).toBe(true);
  });

  it("refuses a remote workspace, whose files are on the probe host", () => {
    expect(canRevealPath(false)).toBe(false);
  });
});
