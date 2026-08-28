// markWebBuild() is a module-global with no un-set, so the browser-build cases
// need their own file — vitest isolates modules per test file. The desktop
// cases live in canReveal.test.ts.
import { describe, it, expect, beforeAll } from "vitest";
import { markWebBuild } from "./hostEnv";
import { canRevealPath } from "./canReveal";

beforeAll(() => {
  markWebBuild();
});

describe("canRevealPath — browser build", () => {
  it("refuses even a local workspace", () => {
    // `reveal_path` resolves to null in a tab, so the affordance is not just
    // broken — it is silent. The only honest UI is no affordance.
    expect(canRevealPath(true)).toBe(false);
  });

  it("still refuses a remote workspace", () => {
    expect(canRevealPath(false)).toBe(false);
  });
});
