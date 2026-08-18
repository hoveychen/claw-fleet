import { describe, it, expect } from "vitest";
import { DSH_FEATURED_VENDORS, dshFindPick, dshModelMenu } from "./modelChoices";
import type { DshModelCatalog } from "./generated/types";

/** Shapes taken from the live `/dsh_models` payload on this machine (dsh
 *  0.1.0-rc.7): a 2-model provider and a 276-model one whose ids carry their own
 *  vendor prefix, with reasoning present, present-without-default, and absent. */
function catalog(): DshModelCatalog {
  const or = (id: string, efforts: string[] = [], defaultEffort: string | null = null) => ({
    id,
    name: id,
    description: null,
    spec: `openrouter/${id}`,
    efforts: efforts.map((e) => ({ id: e, name: e })),
    defaultEffort,
  });
  return {
    groups: [
      {
        id: "deepseek-official",
        name: "DeepSeek",
        models: [
          {
            id: "deepseek-v4-pro",
            name: "DeepSeek-V4-Pro",
            description: null,
            spec: "deepseek-official/deepseek-v4-pro",
            efforts: [
              { id: "off", name: "Off" },
              { id: "high", name: "High" },
            ],
            defaultEffort: "high",
          },
          {
            id: "deepseek-v4-flash",
            name: "DeepSeek-V4-Flash",
            description: null,
            spec: "deepseek-official/deepseek-v4-flash",
            efforts: [{ id: "off", name: "Off" }],
            defaultEffort: "high",
          },
        ],
      },
      {
        id: "openrouter",
        name: "openrouter",
        models: [
          or("ai21/jamba-large-1.7"),
          or("anthropic/claude-opus-5", ["off", "low", "high"], "high"),
          or("anthropic/claude-sonnet-5", ["off", "high"]),
          or("google/gemini-3.6-flash"),
          or("moonshotai/kimi-k3"),
          or("openai/gpt-5.6-sol", ["low", "medium", "high"]),
          or("qwen/qwen3-max"),
          or("z-ai/glm-5"),
          or("bare-id-no-vendor"),
        ],
      },
    ],
    failures: [],
  };
}

/** A group big enough to be folded, so the vendor split is exercised. */
function bigCatalog(): DshModelCatalog {
  const c = catalog();
  const filler = Array.from({ length: 40 }, (_, i) => ({
    id: `qwen/filler-${i}`,
    name: `filler-${i}`,
    description: null,
    spec: `openrouter/qwen/filler-${i}`,
    efforts: [],
    defaultEffort: null,
  }));
  c.groups[1].models = [...c.groups[1].models, ...filler];
  return c;
}

describe("dshModelMenu", () => {
  it("lists a small provider inline and folds a large one by vendor", () => {
    const menu = dshModelMenu(bigCatalog());

    // DeepSeek has 2 models — short enough to read in the popover, so no folder.
    expect(menu.inline.map((p) => p.value)).toEqual([
      "deepseek-official/deepseek-v4-pro",
      "deepseek-official/deepseek-v4-flash",
    ]);

    // openrouter is folded. Featured vendors keep the order the boss picked;
    // the catch-all comes last with everything else.
    const vendors = menu.folders.map((f) => f.vendor);
    expect(vendors.slice(0, -1)).toEqual(
      DSH_FEATURED_VENDORS.filter((v) => v !== "deepseek"),
    );
    expect(vendors[vendors.length - 1]).toBe("");

    const anthropic = menu.folders.find((f) => f.vendor === "anthropic")!;
    expect(anthropic.models.map((p) => p.value)).toEqual([
      "openrouter/anthropic/claude-opus-5",
      "openrouter/anthropic/claude-sonnet-5",
    ]);

    // The catch-all takes the unfeatured vendors *and* the id with no vendor
    // prefix at all — nothing may be silently unreachable.
    const other = menu.folders.find((f) => f.vendor === "")!;
    const otherIds = other.models.map((p) => p.value);
    expect(otherIds).toContain("openrouter/qwen/qwen3-max");
    expect(otherIds).toContain("openrouter/z-ai/glm-5");
    expect(otherIds).toContain("openrouter/ai21/jamba-large-1.7");
    expect(otherIds).toContain("openrouter/bare-id-no-vendor");
  });

  it("offers every catalogue model somewhere — no model is dropped", () => {
    const c = bigCatalog();
    const want = c.groups.flatMap((g) => g.models.map((m) => m.spec)).sort();
    const menu = dshModelMenu(c);
    const got = [
      ...menu.inline.map((p) => p.value),
      ...menu.folders.flatMap((f) => f.models.map((p) => p.value)),
    ].sort();
    expect(got).toEqual(want);
  });

  it("keeps a group inline whole when it is under the fold cap", () => {
    // The sample openrouter group is only 9 models — below the cap, so it is
    // read inline like DeepSeek rather than folded. Folding is about length,
    // not about which provider it is.
    const menu = dshModelMenu(catalog());
    expect(menu.folders).toEqual([]);
    expect(menu.inline).toHaveLength(11);
  });

  it("carries each model's own effort scale, never a shared ladder", () => {
    const menu = dshModelMenu(bigCatalog());

    const pro = dshFindPick(menu, "deepseek-official/deepseek-v4-pro")!;
    expect(pro.efforts).toEqual(["off", "high"]);
    expect(pro.defaultEffort).toBe("high");

    // efforts but no default — dsh decides, so Fleet must not invent one.
    const sonnet = dshFindPick(menu, "openrouter/anthropic/claude-sonnet-5")!;
    expect(sonnet.efforts).toEqual(["off", "high"]);
    expect(sonnet.defaultEffort).toBe("");

    // No reasoning control at all: an empty scale, so the effort pill has
    // nothing but "default" to offer.
    const jamba = dshFindPick(menu, "openrouter/ai21/jamba-large-1.7")!;
    expect(jamba.efforts).toEqual([]);
    expect(jamba.defaultEffort).toBe("");
  });

  it("finds a pick inside a folder, not just the inline ones", () => {
    const menu = dshModelMenu(bigCatalog());
    expect(dshFindPick(menu, "openrouter/qwen/qwen3-max")?.label).toBe("qwen/qwen3-max");
    expect(dshFindPick(menu, "openrouter/nope")).toBeUndefined();
  });

  it("degrades to an empty menu rather than throwing when there is no catalogue", () => {
    // The launcher then shows only its "default" item, which is honest: the
    // session runs on whatever ~/.dsh/settings.yaml selects. A thrown error
    // here would blank the whole pill row.
    for (const empty of [null, undefined, { groups: [], failures: [] }]) {
      const menu = dshModelMenu(empty as DshModelCatalog | null);
      expect(menu.inline).toEqual([]);
      expect(menu.folders).toEqual([]);
    }
  });

  it("still lists the providers it reached when another provider failed", () => {
    // `failures` is a per-provider array that arrives *alongside* a populated
    // `groups`; treating it as fatal would blank a working DeepSeek list
    // because some unrelated route lost its credential.
    const c = catalog();
    c.failures = [{ id: "moonshot", name: "Moonshot", message: "missing credential" }];
    const menu = dshModelMenu(c);
    expect(menu.inline.length).toBeGreaterThan(0);
  });

  it("tolerates a payload whose arrays are missing outright", () => {
    // RemoteBackend talks to someone else's `fleet serve`, which may predate
    // these fields entirely.
    const menu = dshModelMenu({ groups: undefined, failures: undefined } as never);
    expect(menu.inline).toEqual([]);
    expect(menu.folders).toEqual([]);
  });
});
