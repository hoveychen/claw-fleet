import { describe, expect, it } from "vitest";
import {
  classifyResult,
  isFleetTool,
  parseFleetCall,
  parsePlanGet,
  parsePlanList,
  parseWikiList,
  parseWikiSearch,
} from "./fleetTools";

describe("isFleetTool", () => {
  it("matches the MCP-namespaced wire name and the bare name", () => {
    expect(isFleetTool("mcp__fleet__fleet__plan")).toBe("plan");
    expect(isFleetTool("fleet__wiki")).toBe("wiki");
    expect(isFleetTool("mcp__fleet__fleet__handoff")).toBe("handoff");
  });

  it("does not match non-control fleet tools or foreign tools", () => {
    expect(isFleetTool("mcp__fleet__fleet__ask")).toBeNull();
    expect(isFleetTool("Bash")).toBeNull();
    expect(isFleetTool("AskUserQuestion")).toBeNull();
  });
});

describe("parsePlanList", () => {
  it("parses `id [done/total] — source` lines (core format)", () => {
    // Mirrors mcp_control.rs handle_plan `list`.
    const text = "m0-graybox [2/5]\nauth-refactor [1/3] — /Users/x/TASKS.md";
    expect(parsePlanList(text)).toEqual([
      { id: "m0-graybox", done: 2, total: 5, source: undefined },
      { id: "auth-refactor", done: 1, total: 3, source: "/Users/x/TASKS.md" },
    ]);
  });
});

describe("parsePlanGet", () => {
  it("parses `[x]/[ ] text` checklist lines", () => {
    const text = "[x] P1 — done thing\n[ ] P2 — pending thing";
    expect(parsePlanGet(text)).toEqual([
      { done: true, text: "P1 — done thing" },
      { done: false, text: "P2 — pending thing" },
    ]);
  });
});

describe("parseWikiList / parseWikiSearch", () => {
  it("parses `slug  [kind]  vN  title`", () => {
    // core wiki `list`: "{slug}  [{kind}]  v{n}  {title}"
    const text = "arch/overview  [html]  v3  Architecture Overview";
    expect(parseWikiList(text)).toEqual([
      { slug: "arch/overview", kind: "html", versions: "v3", title: "Architecture Overview" },
    ]);
  });

  it("parses `slug  [field]  matched`", () => {
    const text = "promo/reddit  [body]  …subreddit playbook…";
    expect(parseWikiSearch(text)).toEqual([
      { slug: "promo/reddit", field: "body", matched: "…subreddit playbook…" },
    ]);
  });
});

describe("classifyResult", () => {
  it("classifies a mutate confirmation as `confirm`", () => {
    const r = classifyResult("plan", "check", "ok: checked P3 in m0-graybox", false);
    expect(r).toEqual({ kind: "confirm", text: "ok: checked P3 in m0-graybox" });
  });

  it("classifies an errored call as `error`, keeping the message", () => {
    const r = classifyResult("plan", "check", "plan 'x' not found", true);
    expect(r).toEqual({ kind: "error", text: "plan 'x' not found" });
  });

  it("parses JSON list/get for handoff/watch/loop/schedule into `records`", () => {
    // watch list returns serde_json::to_string_pretty(&Vec<WatchRecord>).
    const json = JSON.stringify([{ id: "w1", untilCmd: "test -f done" }]);
    const r = classifyResult("watch", "list", json, false);
    expect(r.kind).toBe("records");
    if (r.kind === "records") {
      expect(r.records).toEqual([{ id: "w1", untilCmd: "test -f done" }]);
    }
  });

  it("wraps a single JSON object (loop get) as a one-element records array", () => {
    const json = JSON.stringify({ id: "l1", prompt: "poll", intervalSecs: 300 });
    const r = classifyResult("loop", "get", json, false);
    expect(r.kind).toBe("records");
    if (r.kind === "records") expect(r.records).toHaveLength(1);
  });

  it("falls back to `raw` when a list return isn't parseable JSON", () => {
    const r = classifyResult("schedule", "list", "no schedules registered", false);
    expect(r).toEqual({ kind: "raw", text: "no schedules registered" });
  });

  it("routes plan list/get to their line parsers", () => {
    expect(classifyResult("plan", "list", "m0-graybox [2/5]", false).kind).toBe("plan-list");
    expect(classifyResult("plan", "get", "[x] P1", false).kind).toBe("plan-get");
  });

  it("treats wiki cat as the raw document body", () => {
    const r = classifyResult("wiki", "cat", "# Title\n\nbody", false);
    expect(r).toEqual({ kind: "wiki-cat", body: "# Title\n\nbody" });
  });

  it("returns `none` for an empty result", () => {
    expect(classifyResult("plan", "check", "   ", false)).toEqual({ kind: "none" });
  });
});

describe("parseFleetCall", () => {
  it("extracts action + classifies result end to end", () => {
    const view = parseFleetCall(
      "plan",
      { action: "check", plan_id: "m0-graybox", task: "P3" },
      "ok: checked P3",
      false,
    );
    expect(view.tool).toBe("plan");
    expect(view.action).toBe("check");
    expect(view.result).toEqual({ kind: "confirm", text: "ok: checked P3" });
  });
});
