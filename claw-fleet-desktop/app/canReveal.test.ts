// Desktop case. The browser-build one needs markWebBuild(), which cannot be
// undone within a module graph — it lives in canReveal.web.test.ts.
import { describe, it, expect } from "vitest";
import { canRevealPath } from "./canReveal";

describe("canRevealPath — desktop", () => {
  it("allows revealing: the files are on this machine", () => {
    expect(canRevealPath()).toBe(true);
  });
});
