import { describe, expect, it } from "vitest";
import { formatBoundaryDetail } from "./ErrorBoundary";

// Crash details are the only clues we have afterward (no console on phone). Pin the shape here, so we don't
// "conveniently simplify" away componentStack or stack someday — without componentStack we don't know
// which component threw, and that's exactly the critical piece of info in both blank-screen incidents.
//
// The component's own behavior (capture, blast radius, resetKey auto-recovery) isn't tested here: this
// package's test environment is Node, no jsdom / testing-library, and error boundary doesn't work under SSR.
// That part goes through P3 browser end-to-end (real relay + malformed card), closer to reality than installing two new deps.
describe("formatBoundaryDetail", () => {
  it("Includes error name, message, stack, and component stack", () => {
    const e = new TypeError("x.filter is not a function");
    e.stack = "TypeError: x.filter is not a function\n    at toolChoicesForSources";
    const out = formatBoundaryDetail(e, "\n    at NewSessionSheet\n    at App");

    expect(out).toContain("TypeError: x.filter is not a function");
    expect(out).toContain("at toolChoicesForSources");
    expect(out).toContain("Component stack:");
    expect(out).toContain("at NewSessionSheet");
  });

  it("When no component stack, don't leave an empty Component stack section", () => {
    const e = new Error("boom");
    e.stack = "Error: boom";
    const out = formatBoundaryDetail(e, null);
    expect(out).not.toContain("Component stack");
    expect(out).toContain("Error: boom");
  });

  it("When stack is missing, explicitly mark it instead of rendering undefined", () => {
    const e = new Error("no stack here");
    e.stack = undefined;
    expect(formatBoundaryDetail(e, null)).toContain("(no stack)");
  });
});
