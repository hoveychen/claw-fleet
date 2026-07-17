import { describe, expect, it } from "vitest";
import { patchToolSummary } from "./toolSummary";

const tr = (key: string, ...args: Array<string | number>) =>
  args.reduce((out, value, index) => out.replace(`{${index}}`, String(value)), key);

describe("patchToolSummary", () => {
  it("names one added, updated, or deleted file without dumping the patch", () => {
    expect(patchToolSummary("*** Begin Patch\n*** Add File: src/new.ts\n*** End Patch", tr))
      .toBe("新建 new.ts");
    expect(patchToolSummary("*** Begin Patch\n*** Update File: src/old.ts\n*** End Patch", tr))
      .toBe("编辑 old.ts");
    expect(patchToolSummary("*** Begin Patch\n*** Delete File: src/gone.ts\n*** End Patch", tr))
      .toBe("删除 gone.ts");
  });

  it("counts multi-file patches", () => {
    const patch = `*** Begin Patch
*** Update File: a.ts
*** Add File: b.ts
*** Delete File: c.ts
*** End Patch`;
    expect(patchToolSummary(patch, tr)).toBe("编辑 3 个文件");
  });

  it("returns null for a non-patch command", () => {
    expect(patchToolSummary("const result = 1", tr)).toBeNull();
  });
});
