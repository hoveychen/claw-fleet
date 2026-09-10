import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";

import { describe, expect, it } from "vitest";
import {
  FLEET_CONTROL_TOOLS,
  FLEET_TOOL_LABEL_KEYS,
  classifyResult,
  friendlyToolName,
  isFleetTool,
  isIngestCall,
  parseArtifactAdd,
  parseFleetCall,
  parseWikiPublish,
  parsePlanGet,
  parsePlanList,
  parseWikiList,
  parseWikiSearch,
  resultText,
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

// ── the drift this file now guards ──────────────────────────────────────────
//
// `FLEET_CONTROL_TOOLS` had six entries while core advertised eleven. The five
// missing tools (artifact / inspect / control / notes / history) failed
// `isFleetTool`, so ContentBlocks never routed them to FleetToolCard and they
// fell through to the generic card's `JSON.stringify(input)` last resort — and
// in the work-run rail, where the tool name is hidden, that blob was the whole
// row: `{"action":"list","all":true}` and nothing else. These two tests read the
// Rust registries rather than a hardcoded list, so the next tool added to core
// reddens here instead of shipping as a raw-JSON row.

const CORE_SRC = resolve(__dirname, "../../../../claw-fleet-core/src");

/** Pull the string literals out of a `pub const NAME: [&str; N] = [...]`. */
function rustStrArray(file: string, constName: string): string[] {
  const src = readFileSync(join(CORE_SRC, file), "utf8");
  const m = new RegExp(`${constName}[^=]*=\\s*\\[([^\\]]*)\\]`).exec(src);
  if (!m) throw new Error(`${constName} not found in ${file}`);
  return [...m[1].matchAll(/"([^"]+)"/g)].map((x) => x[1]);
}

describe("registry parity with claw-fleet-core", () => {
  it("FLEET_CONTROL_TOOLS mirrors CONTROL_TOOL_NAMES", () => {
    const core = rustStrArray("mcp_control.rs", "CONTROL_TOOL_NAMES");
    expect([...FLEET_CONTROL_TOOLS].map((t) => `fleet__${t}`)).toEqual(core);
  });

  it("every Fleet MCP tool has a label key (control + always-on)", () => {
    const names = [
      ...rustStrArray("mcp_control.rs", "CONTROL_TOOL_NAMES"),
      ...rustStrArray("mcp_server.rs", "ALWAYS_ON_TOOL_NAMES"),
    ];
    const missing = names.filter((n) => !(n.replace(/^fleet__/, "") in FLEET_TOOL_LABEL_KEYS));
    expect(missing).toEqual([]);
  });
});

describe("friendlyToolName", () => {
  const t = (key: string) => key;

  it("labels the tools that used to leak as `fleet·fleet__<tail>`", () => {
    // The screenshot case: a ToolSearch `select:` list naming fleet__inspect.
    expect(friendlyToolName("mcp__fleet__fleet__inspect", t)).toBe("detail.fleet_tool.inspect");
    expect(friendlyToolName("fleet__notes", t)).toBe("detail.fleet_tool.notes");
    expect(friendlyToolName("mcp__fleet__fleet__history", t)).toBe("detail.fleet_tool.history");
  });

  it("does not let the `image` entry swallow `image_edit`", () => {
    expect(friendlyToolName("mcp__fleet__fleet__image", t)).toBe("detail.fleet_tool.image");
    expect(friendlyToolName("mcp__fleet__fleet__image_edit", t)).toBe(
      "detail.fleet_tool.image_edit",
    );
  });

  it("still renders a foreign MCP tool as server·tool", () => {
    expect(friendlyToolName("mcp__linear__create_issue", t)).toBe("linear·create_issue");
  });
});

describe("classifyResult for the prose-returning tools", () => {
  // mcp_inspect.rs `list` returns "N agent(s)\n  <row>\n…" — multi-line, and
  // the pre-existing catch-all sent every unrecognised action to `confirm`,
  // whose single-line div would have run those rows together.
  it("keeps inspect's multi-line agent table as raw text", () => {
    const table = "2 agent(s)\n  a1b2  claude-fleet  running\n  c3d4  netferry  idle";
    expect(classifyResult("inspect", "list", table, false)).toEqual({ kind: "raw", text: table });
    expect(classifyResult("inspect", "account", "me <a@b.c>\n  plan: team", false).kind).toBe("raw");
  });

  it("keeps history/notes/artifact reads raw and their mutates as confirms", () => {
    expect(classifyResult("history", "search", "3 hit(s) for 'x'\n  abc line 9", false).kind).toBe("raw");
    expect(classifyResult("notes", "list", "checkpoint.md  120 bytes", false).kind).toBe("raw");
    expect(classifyResult("notes", "write", "ok: write checkpoint.md (120 bytes)", false)).toEqual({
      kind: "confirm",
      text: "ok: write checkpoint.md (120 bytes)",
    });
    expect(classifyResult("artifact", "list", "id  title  [pdf]  10 bytes  ws", false).kind).toBe("raw");
    expect(classifyResult("artifact", "delete", "Deleted artifact a1.", false).kind).toBe("confirm");
  });

  it("renders a note body like a wiki document", () => {
    // `notes read` returns the file verbatim, the same shape as `wiki cat`.
    const body = "# checkpoint\n\n- goal: x";
    expect(classifyResult("notes", "read", body, false)).toEqual({ kind: "wiki-cat", body });
  });

  it("treats a control signal's reply as a confirmation", () => {
    const ok = "ok: sent SIGTERM to a1b2 (claude-fleet) pid 42 and its process tree";
    expect(classifyResult("control", "stop", ok, false)).toEqual({ kind: "confirm", text: ok });
  });

  it("still surfaces an error result as an error for the new tools", () => {
    expect(classifyResult("inspect", "get", "no such agent", true).kind).toBe("error");
  });
});

describe("isFleetTool for the five tools that were missing", () => {
  it("matches artifact / inspect / control / notes / history", () => {
    expect(isFleetTool("mcp__fleet__fleet__inspect")).toBe("inspect");
    expect(isFleetTool("mcp__fleet__fleet__control")).toBe("control");
    expect(isFleetTool("mcp__fleet__fleet__notes")).toBe("notes");
    expect(isFleetTool("mcp__fleet__fleet__history")).toBe("history");
    expect(isFleetTool("fleet__artifact")).toBe("artifact");
  });

  it("still leaves the non-control tools to the generic card", () => {
    expect(isFleetTool("mcp__fleet__fleet__image")).toBeNull();
    expect(isFleetTool("mcp__fleet__fleet__set_session_title")).toBeNull();
  });
});

// ── ingest confirmations (artifact add / wiki publish) ──────────────────────
//
// These two mutates are the only ones whose subject the reader wants to *see*.
// Core returns them as one prose sentence, so the id/slug the card needs to
// resolve the real thing has to be recovered from that sentence — hence a
// parser, and hence the format-string drift guard at the bottom of this block.

describe("parseArtifactAdd", () => {
  it("recovers id, title, kind and size from the confirmation line", () => {
    const line =
      "Stored artifact 20260909-080326 — 9/8 对外更新日志 (markdown, 12345 bytes), copied. " +
      "It is now on the 产出 page.";
    expect(parseArtifactAdd(line)).toEqual({
      id: "20260909-080326",
      title: "9/8 对外更新日志",
      artifactKind: "markdown",
      bytes: 12345,
    });
  });

  it("keeps a title that itself contains a parenthesis", () => {
    const line = "Stored artifact a1 — Q3 报表 (最终版) (xlsx, 9 bytes), hard-linked.";
    expect(parseArtifactAdd(line)?.title).toBe("Q3 报表 (最终版)");
    expect(parseArtifactAdd(line)?.artifactKind).toBe("xlsx");
  });

  it("returns null on an unrecognised sentence", () => {
    expect(parseArtifactAdd("Deleted artifact a1.")).toBeNull();
  });
});

describe("parseWikiPublish", () => {
  it("recovers slug, version and title", () => {
    expect(parseWikiPublish("Published arch/overview (version v3, 3 total). title: 架构总览")).toEqual({
      slug: "arch/overview",
      version: "v3",
      title: "架构总览",
    });
  });

  it("returns null on an unrecognised sentence", () => {
    expect(parseWikiPublish("Moved arch/overview to arch/v2/overview.")).toBeNull();
  });
});

describe("classifyResult routes the two ingests to their own kinds", () => {
  const stored = "Stored artifact a1 — Report (pdf, 9 bytes), copied. It is now on the 产出 page.";

  it("classifies artifact add and wiki publish structurally", () => {
    const a = classifyResult("artifact", "add", stored, false);
    expect(a).toEqual({
      kind: "artifact-add",
      artifact: { id: "a1", title: "Report", artifactKind: "pdf", bytes: 9 },
    });
    expect(classifyResult("wiki", "publish", "Published s (version v1, 1 total). title: T", false).kind).toBe(
      "wiki-publish",
    );
  });

  it("falls back to the plain confirm line when the wording is unrecognised", () => {
    expect(classifyResult("artifact", "add", "stored it somewhere", false).kind).toBe("confirm");
    expect(classifyResult("wiki", "publish", "published it", false).kind).toBe("confirm");
  });

  it("still surfaces a failed ingest as an error", () => {
    expect(classifyResult("artifact", "add", "no such file", true).kind).toBe("error");
  });
});

describe("isIngestCall", () => {
  it("matches exactly the two calls that put something in a store", () => {
    expect(isIngestCall("mcp__fleet__fleet__artifact", { action: "add" })).toBe(true);
    expect(isIngestCall("fleet__wiki", { action: "publish" })).toBe(true);
  });

  it("rejects the same tools' read and delete actions, and other tools", () => {
    expect(isIngestCall("mcp__fleet__fleet__artifact", { action: "list" })).toBe(false);
    expect(isIngestCall("mcp__fleet__fleet__artifact", { action: "delete" })).toBe(false);
    expect(isIngestCall("mcp__fleet__fleet__wiki", { action: "cat" })).toBe(false);
    expect(isIngestCall("mcp__fleet__fleet__plan", { action: "add" })).toBe(false);
    expect(isIngestCall("Read", { action: "add" })).toBe(false);
    expect(isIngestCall("mcp__fleet__fleet__artifact", null)).toBe(false);
  });
});

describe("resultText", () => {
  // The regression that hid every Fleet card's result for months: an MCP tool
  // never returns a bare string, only a block array, and the old one-liner
  // dropped that shape on the floor. Every test above hand-fed a string, so
  // the suite stayed green while the app showed empty cards.
  it("reads the block-array shape MCP actually returns", () => {
    const content = [{ type: "text", text: "Stored artifact 20260909-211340 — x (image, 1 bytes)" }];
    expect(resultText(content)).toBe("Stored artifact 20260909-211340 — x (image, 1 bytes)");
  });

  it("concatenates multiple text blocks and skips non-text ones", () => {
    expect(resultText([{ type: "text", text: "a" }, { type: "image" }, { type: "text", text: "b" }])).toBe("ab");
  });

  it("still passes a plain string through", () => {
    expect(resultText("ok: done")).toBe("ok: done");
  });

  it("survives an empty or malformed array", () => {
    expect(resultText([])).toBe("");
    expect(resultText([null, 42] as unknown[])).toBe("");
  });
});

describe("an ingest arriving in MCP's real block-array shape", () => {
  // End-to-end over the two functions FleetToolCard composes: the exact
  // tool_result content from a real transcript must reach `artifact-add`.
  it("classifies through resultText into artifact-add", () => {
    const content = [
      {
        type: "text",
        text:
          "Stored artifact 20260909-211340 — 教师节海报 · 生成模型暖光教室版 " +
          "(image, 857146 bytes), hard-linked. It is now on the 产出 page.",
      },
    ];
    const view = parseFleetCall("artifact", { action: "add" }, resultText(content), false);
    expect(view.result.kind).toBe("artifact-add");
    if (view.result.kind !== "artifact-add") return;
    expect(view.result.artifact.id).toBe("20260909-211340");
    expect(view.result.artifact.artifactKind).toBe("image");
    expect(view.result.artifact.bytes).toBe(857146);
  });
});

describe("ingest wording parity with claw-fleet-core", () => {
  // The parsers above read a *sentence*, so a reworded format string in core
  // would not fail any type check — the cards would just quietly go back to
  // showing one line of gray text. Assert the literals instead.
  const src = readFileSync(join(CORE_SRC, "mcp_control.rs"), "utf8");

  it("artifact add still returns `Stored artifact {} — {} ({}, {} bytes)`", () => {
    expect(src).toContain("Stored artifact {} — {} ({}, {} bytes)");
  });

  it("wiki publish still returns `Published {} (version {}, {} total). title: {}`", () => {
    expect(src).toContain("Published {} (version {}, {} total). title: {}");
  });
});
