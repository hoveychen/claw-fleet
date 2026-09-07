import { describe, expect, it } from "vitest";

import { versionCheckArgs } from "./versionCheck";

describe("versionCheckArgs", () => {
  it("keeps automatic checks cacheable and passes the active locale", () => {
    expect(versionCheckArgs(false, "zh-CN")).toEqual({
      force: false,
      locale: "zh-CN",
    });
  });

  it("forces menu checks and falls back to English when locale is absent", () => {
    expect(versionCheckArgs(true)).toEqual({
      force: true,
      locale: "en",
    });
  });
});
