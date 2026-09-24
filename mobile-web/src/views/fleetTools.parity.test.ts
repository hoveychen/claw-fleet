import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";

import { describe, expect, it } from "vitest";

import { FLEET_CONTROL_TOOLS } from "./fleetTools";
import { fleetSummary } from "./FleetBody";
import { friendlyToolName } from "./toolSummary";

// `fleet__spawn` never made it into this list, so a spawn call failed
// `isFleetTool` and rendered as a raw `fleet·fleet__spawn` row; `plan snooze`,
// `wiki mv`, `control send` … had no summary case and showed the bare English
// action in the Chinese UI. Read the Rust registries so the next tool or
// action added to core reddens here.

const CORE_SRC = resolve(__dirname, "../../../claw-fleet-core/src");

function controlToolDefs(): Array<{ tool: string; actions: string[] }> {
  const src = ["mcp_control.rs", "mcp_inspect.rs"].map((f) => readFileSync(join(CORE_SRC, f), "utf8")).join("\n");
  return [...src.matchAll(/"name":\s*"fleet__(\w+)"([\s\S]*?)"additionalProperties"/g)].map(([, tool, body]) => {
    const m = /"action":\s*\{"type":\s*"string",\s*"enum":\s*\[([^\]]*)\]/.exec(body);
    return { tool, actions: m ? [...m[1].matchAll(/"(\w+)"/g)].map((x) => x[1]) : [] };
  });
}

describe("fleet control tool parity with claw-fleet-core", () => {
  it("FLEET_CONTROL_TOOLS mirrors CONTROL_TOOL_NAMES", () => {
    const src = readFileSync(join(CORE_SRC, "mcp_control.rs"), "utf8");
    const m = /CONTROL_TOOL_NAMES[^=]*=\s*\[([^\]]*)\]/.exec(src);
    const core = [...m![1].matchAll(/"([^"]+)"/g)].map((x) => x[1]);
    expect([...FLEET_CONTROL_TOOLS].map((t) => `fleet__${t}`)).toEqual(core);
  });

  it("every control tool has a friendly label", () => {
    const raw = controlToolDefs()
      .map((d) => d.tool)
      .filter((t) => friendlyToolName(`mcp__fleet__fleet__${t}`).includes("fleet__"));
    expect(raw).toEqual([]);
  });

  it("every schema action has a summary instead of the bare action", () => {
    const defs = controlToolDefs();
    expect(defs.length).toBeGreaterThanOrEqual(FLEET_CONTROL_TOOLS.length);
    const bare: string[] = [];
    for (const { tool, actions } of defs) {
      for (const action of actions.length ? actions : [""]) {
        const s = fleetSummary(tool as (typeof FLEET_CONTROL_TOOLS)[number], action ? { action } : {});
        if (s === (action || tool)) bare.push(`${tool}.${action || "(none)"}`);
      }
    }
    expect(bare).toEqual([]);
  });
});
