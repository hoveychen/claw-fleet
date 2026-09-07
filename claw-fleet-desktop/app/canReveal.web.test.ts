// markWebBuild() is a module-global with no un-set, so the browser-build case
// needs its own file — vitest isolates modules per test file. The desktop
// case lives in canReveal.test.ts.
import { describe, it, expect, beforeAll } from "vitest";
import { markWebBuild } from "./hostEnv";
import { canRevealPath } from "./canReveal";

beforeAll(() => {
  markWebBuild();
});

describe("canRevealPath — browser build", () => {
  it("refuses: a tab cannot open a file manager", () => {
    // `reveal_path` resolves to null in a tab, so the affordance is not just
    // broken — it is silent. The only honest UI is no affordance.
    expect(canRevealPath()).toBe(false);
  });
});
