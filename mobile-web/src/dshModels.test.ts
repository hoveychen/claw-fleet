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

/** 线上形状:一个小 group(DeepSeek 2 个) + 一个大 group(openrouter,超过阈值)。 */
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
  it("小 group 整组平铺,不折子分组", () => {
    const groups = dshModelGroups(catalog(DSH_INLINE_GROUP_CAP + 1));
    const deepseek = groups.find((g) => g.label === "DeepSeek");
    expect(deepseek?.models.map(([v]) => v)).toEqual(["x/chat", "x/reasoner"]);
  });

  it("大 group 按 vendor 拆,featured 的排在前", () => {
    const groups = dshModelGroups(catalog(DSH_INLINE_GROUP_CAP + 1));
    const labels = groups.map((g) => g.label);
    expect(labels.some((l) => l.includes("anthropic"))).toBe(true);
    expect(labels.indexOf("DeepSeek")).toBeLessThan(
      labels.findIndex((l) => l.includes("anthropic")),
    );
  });

  it("全覆盖:目录里每个模型都恰好出现一次", () => {
    const c = catalog(DSH_INLINE_GROUP_CAP + 1);
    const all = c.groups.flatMap((g) => g.models.map((m) => m.spec)).sort();
    const emitted = dshModelGroups(c)
      .flatMap((g) => g.models.map(([v]) => v))
      .sort();
    expect(emitted).toEqual(all);
  });

  it("目录缺失 / 为空 → 空数组,不抛", () => {
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

  it("给出选中模型自己的阶梯和 dsh 的默认档", () => {
    const r = dshEffortsFor(c, "x/thinker");
    expect(r.efforts.map(([v]) => v)).toEqual(["low", "high"]);
    expect(r.defaultEffort).toBe("high");
  });

  it("没有推理控制的模型 / 认不出的 spec → 空阶梯", () => {
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

  it("模型停在「默认」时,阶梯跟目录里 dsh 的默认模型走", () => {
    // 老板反馈:一直用默认模型的机器上 effort 下拉永远只有「默认」一项。
    expect(dshLadderSpec(c, "")).toBe("x/thinker");
    const r = dshEffortsFor(c, dshLadderSpec(c, ""));
    expect(r.efforts.map(([v]) => v)).toEqual(["low", "high"]);
    expect(r.defaultEffort).toBe("high");
  });

  it("显式选了模型就用它,不看默认", () => {
    expect(dshLadderSpec(c, "x/plain")).toBe("x/plain");
    expect(dshEffortsFor(c, dshLadderSpec(c, "x/plain")).efforts).toEqual([]);
  });

  it("目录缺失 / 没有默认 / 老主机没这个字段 → \"\",下游当没有阶梯", () => {
    expect(dshLadderSpec(null, "")).toBe("");
    expect(dshLadderSpec({ ...c, defaultSpec: null }, "")).toBe("");
    expect(dshLadderSpec({ groups: [], failures: [] } as never, "")).toBe("");
    expect(dshEffortsFor(c, dshLadderSpec(null, "")).efforts).toEqual([]);
  });
});
