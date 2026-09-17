import { describe, expect, it } from "vitest";
import { DSH_INLINE_GROUP_CAP, dshEffortsFor, dshLadderSpec, dshModelGroups } from "./dshModels";
import type { DshModelCatalog } from "./generated/types";

const model = (id: string, efforts: string[] = [], defaultEffort: string | null = null) => ({
  id,
  name: id,
  description: null,
  spec: `x/${id}`,
  efforts: efforts.map((e) => ({ id: e, name: e })),
  defaultEffort,
});

/** Production shape: one small group (DeepSeek 2 models) + one large group (OpenRouter, over threshold). */
const catalog = (bigCount: number): DshModelCatalog => ({
  groups: [
    { id: "deepseek", name: "DeepSeek", models: [model("chat"), model("reasoner")] },
    {
      id: "openrouter",
      name: "OpenRouter",
      models: [
        ...Array.from({ length: bigCount }, (_, i) => model(`anthropic/m${i}`)),
        model("zzz-unfeatured/m0"),
      ],
    },
  ],
  failures: [],
  defaultSpec: null,
  defaultEffort: null,
});

describe("dshModelGroups", () => {
  it("small group flattened entirely, no subgrouping", () => {
    const groups = dshModelGroups(catalog(DSH_INLINE_GROUP_CAP + 1));
    const deepseek = groups.find((g) => g.label === "DeepSeek");
    expect(deepseek?.models.map(([v]) => v)).toEqual(["x/chat", "x/reasoner"]);
  });

  it("large group splits by vendor, featured ones first", () => {
    const groups = dshModelGroups(catalog(DSH_INLINE_GROUP_CAP + 1));
    const labels = groups.map((g) => g.label);
    expect(labels.some((l) => l.includes("anthropic"))).toBe(true);
    expect(labels.indexOf("DeepSeek")).toBeLessThan(
      labels.findIndex((l) => l.includes("anthropic")),
    );
  });

  it("full coverage: each model in the catalog appears exactly once", () => {
    const c = catalog(DSH_INLINE_GROUP_CAP + 1);
    const all = c.groups.flatMap((g) => g.models.map((m) => m.spec)).sort();
    const emitted = dshModelGroups(c)
      .flatMap((g) => g.models.map(([v]) => v))
      .sort();
    expect(emitted).toEqual(all);
  });

  it("catalog missing / empty → empty array, don't throw", () => {
    expect(dshModelGroups(null)).toEqual([]);
    expect(
      dshModelGroups({ groups: [], failures: [], defaultSpec: null, defaultEffort: null }),
    ).toEqual([]);
  });
});

describe("dshEffortsFor", () => {
  const c: DshModelCatalog = {
    groups: [
      {
        id: "g",
        name: "G",
        models: [model("thinker", ["low", "high"], "high"), model("plain")],
      },
    ],
    failures: [],
    defaultSpec: "x/thinker",
    defaultEffort: "high",
  };

  it("returns the selected model's own ladder and dsh's default level", () => {
    const r = dshEffortsFor(c, "x/thinker");
    expect(r.efforts.map(([v]) => v)).toEqual(["low", "high"]);
    expect(r.defaultEffort).toBe("high");
  });

  it("model without reasoning control / unknown spec → empty ladder", () => {
    expect(dshEffortsFor(c, "x/plain").efforts).toEqual([]);
    expect(dshEffortsFor(c, "x/nope").efforts).toEqual([]);
    expect(dshEffortsFor(null, "x/thinker").efforts).toEqual([]);
  });
});

describe("dshLadderSpec", () => {
  const c: DshModelCatalog = {
    groups: [
      {
        id: "g",
        name: "G",
        models: [model("thinker", ["low", "high"], "high"), model("plain")],
      },
    ],
    failures: [],
    defaultSpec: "x/thinker",
    defaultEffort: "high",
  };

  it("model on 'default', ladder follows dsh's default model in the catalog", () => {
    // User feedback: machines always using the default model see only "default" in the effort dropdown.
    expect(dshLadderSpec(c, "")).toBe("x/thinker");
    const r = dshEffortsFor(c, dshLadderSpec(c, ""));
    expect(r.efforts.map(([v]) => v)).toEqual(["low", "high"]);
    expect(r.defaultEffort).toBe("high");
  });

  it("explicitly picked model, use it, ignore default", () => {
    expect(dshLadderSpec(c, "x/plain")).toBe("x/plain");
    expect(dshEffortsFor(c, dshLadderSpec(c, "x/plain")).efforts).toEqual([]);
  });

  it("catalog missing / no default / old host without the field → empty string, downstream sees no ladder", () => {
    expect(dshLadderSpec(null, "")).toBe("");
    expect(dshLadderSpec({ ...c, defaultSpec: null }, "")).toBe("");
    expect(dshLadderSpec({ groups: [], failures: [] } as never, "")).toBe("");
    expect(dshEffortsFor(c, dshLadderSpec(null, "")).efforts).toEqual([]);
  });
});
