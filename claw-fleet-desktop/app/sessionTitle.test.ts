import { describe, expect, it } from "vitest";
import { preferredSessionTitle } from "./types";
import type { SessionInfo } from "./types";

function session(overrides: Partial<SessionInfo> = {}): SessionInfo {
  return {
    aiTitle: "扫描标题",
    titleOverride: null,
    ...overrides,
  } as SessionInfo;
}

describe("preferredSessionTitle", () => {
  it("prefers the explicit session title over the scanner title", () => {
    expect(
      preferredSessionTitle(
        session({ titleOverride: "模型设置的标题", aiTitle: "首条用户输入" }),
      ),
    ).toBe("模型设置的标题");
  });

  it("falls back to the scanner title when no override exists", () => {
    expect(preferredSessionTitle(session())).toBe("扫描标题");
  });

  it("returns null when neither title source exists", () => {
    expect(
      preferredSessionTitle(session({ titleOverride: null, aiTitle: null })),
    ).toBeNull();
  });
});
