import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";

import { describe, expect, it } from "vitest";
import type { TFunction } from "i18next";

import en from "../../locales/en.json";
import zh from "../../locales/zh.json";
import { paramLabel } from "./FleetToolCard";

const CORE_SRC = resolve(__dirname, "../../../../claw-fleet-core/src");

/** Minimal stand-in for i18next's array-key lookup: first key that resolves. */
function translator(bundle: unknown): TFunction {
  const lookup = (path: string): string | undefined => {
    let node: unknown = bundle;
    for (const seg of path.split(".")) {
      if (typeof node !== "object" || node === null) return undefined;
      node = (node as Record<string, unknown>)[seg];
    }
    return typeof node === "string" ? node : undefined;
  };
  return ((keys: string | string[], opts?: { defaultValue?: string }) => {
    for (const k of Array.isArray(keys) ? keys : [keys]) {
      const hit = lookup(k);
      if (hit !== undefined) return hit;
    }
    return opts?.defaultValue ?? (Array.isArray(keys) ? keys[0] : keys);
  }) as unknown as TFunction;
}

describe("paramLabel", () => {
  const t = translator(zh);

  // The bug this exists for: `fleet.param` is keyed by param name alone, so the
  // one entry for `note` (written for a handoff briefing) captioned an
  // artifact's blurb 「交接」 and a watch's subject 「交接」 too.
  it("gives the three tools that take a `note` three different labels", () => {
    expect(paramLabel(t, "handoff", "note")).toBe("交接便条");
    expect(paramLabel(t, "watch", "note")).toBe("在等什么");
    expect(paramLabel(t, "artifact", "note")).toBe("说明");
  });

  it("falls back to the flat table when a tool has no override", () => {
    expect(paramLabel(t, "plan", "title")).toBe(zh.fleet.param.title);
    expect(paramLabel(t, "artifact", "path")).toBe(zh.fleet.param.path);
  });

  it("falls back to the raw key for a param no table knows", () => {
    expect(paramLabel(t, "control", "brand_new_param")).toBe("brand_new_param");
  });

  it("resolves the same three overrides in English", () => {
    const te = translator(en);
    expect(paramLabel(te, "handoff", "note")).toBe("Briefing");
    expect(paramLabel(te, "watch", "note")).toBe("Waiting for");
    expect(paramLabel(te, "artifact", "note")).toBe("Note");
  });
});

describe("`note` override coverage against claw-fleet-core", () => {
  // A fourth tool growing a `note` param would silently inherit the flat
  // 「交接」 again. Count the declarations in the tool schemas instead of
  // trusting that anyone remembers to come back here.
  it("covers every tool whose MCP schema declares a `note`", () => {
    const src = readFileSync(join(CORE_SRC, "mcp_control.rs"), "utf8");
    const declarations = src.match(/^\s*"note": \{"type": "string"/gm) ?? [];
    expect(declarations.length).toBe(Object.keys(zh.fleet.param_for).length);
  });
});
