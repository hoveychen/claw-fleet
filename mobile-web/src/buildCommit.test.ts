import { describe, expect, it } from "vitest";

import { shortBuildCommit } from "./buildCommit";

/**
 * Logic for retrieving the build commit shown in "About". The important thing to pin down is
 * the empty-value side: when vite's define can't get git info, it passes the string "unknown",
 * which renders as "build unknown" below the version number, looking like the build failed, when
 * actually it just means this bundle wasn't built from git. Empty string prevents the entire line
 * from rendering.
 */
describe("shortBuildCommit", () => {
  it("returns first 7 characters", () => {
    expect(shortBuildCommit("b4372394f0e1c2d3")).toBe("b437239");
  });

  it("returns unchanged if already less than 7 characters", () => {
    expect(shortBuildCommit("abc12")).toBe("abc12");
  });

  it("\"unknown\" is not a commit, returns empty string", () => {
    expect(shortBuildCommit("unknown")).toBe("");
  });

  it("empty string and undefined both return empty string", () => {
    expect(shortBuildCommit("")).toBe("");
    expect(shortBuildCommit(undefined)).toBe("");
  });
});
