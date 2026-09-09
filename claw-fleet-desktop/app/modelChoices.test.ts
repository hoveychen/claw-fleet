import { describe, expect, it } from "vitest";
import {
  agentToolsForSources,
  codexProfileChoices,
  effortChoicesFor,
  harnessAvailable,
  modelChoicesFor,
  toolForAgentSource,
  tokenPanelForAgentSource,
  type CodexProfile,
} from "./modelChoices";

const profile = (p: Partial<CodexProfile> & { name: string }): CodexProfile => ({
  model: null,
  model_provider: null,
  reasoning_effort: null,
  ...p,
});

describe("codexProfileChoices", () => {
  it("encodes the profile marker the backend splits into `-p <name>`", () => {
    const [choice] = codexProfileChoices([
      profile({
        name: "deepseek-flash",
        model: "deepseek/deepseek-v4-flash",
        model_provider: "openrouter",
      }),
    ]);
    // The raw model id must NOT be the value: `-p` carries both the model and
    // its provider, and sending the bare id would route at the default one.
    expect(choice.value).toBe("profile:deepseek-flash");
    expect(choice.label).toBe("deepseek/deepseek-v4-flash (openrouter)");
  });

  it("labels by model id alone when the profile names no provider", () => {
    const [choice] = codexProfileChoices([
      profile({ name: "local", model: "qwen3-coder" }),
    ]);
    expect(choice.label).toBe("qwen3-coder");
  });

  it("falls back to the profile name when it sets no model", () => {
    // A profile may layer only effort/sandbox settings; it is still selectable,
    // it just has nothing better than its own name to show.
    const [choice] = codexProfileChoices([profile({ name: "careful" })]);
    expect(choice.value).toBe("profile:careful");
    expect(choice.label).toBe("careful");
  });

  it("returns nothing when the host has no profiles", () => {
    expect(codexProfileChoices([])).toEqual([]);
  });

});

describe("catalog-driven choices", () => {
  // A catalog shaped like the real `model_catalog` payload. The ladders differ
  // per model on purpose — that is the property the old hardcoded table got
  // wrong, and the reason these choices are derived rather than written down.
  const catalog = [
    {
      name: "claude",
      available: true,
      models: [
        {
          id: "claude-opus-5",
          label: "Opus 5",
          harness: "claude",
          tier: "premium",
          efforts: ["low", "medium", "high", "xhigh", "max"],
          defaultEffort: null,
        },
      ],
    },
    {
      name: "codex",
      available: false,
      models: [
        {
          id: "gpt-5.6-sol",
          label: "GPT-5.6 Sol",
          harness: "codex",
          tier: "premium",
          efforts: ["low", "medium", "high", "xhigh", "max", "ultra"],
          defaultEffort: "medium",
        },
        {
          id: "gpt-5.5",
          label: "GPT-5.5",
          harness: "codex",
          tier: "premium",
          efforts: ["low", "medium", "high", "xhigh"],
          defaultEffort: "xhigh",
        },
      ],
    },
  ];

  it("maps a harness's models onto picker entries", () => {
    expect(modelChoicesFor(catalog, "claude")).toEqual([
      { value: "claude-opus-5", label: "Opus 5" },
    ]);
    expect(modelChoicesFor(catalog, "dsh")).toEqual([]);
  });

  it("gives the picked model's own ladder, not the harness's", () => {
    // The whole point: `gpt-5.5` stops at xhigh while its sibling reaches ultra.
    // The table this replaced had one special case for Astra and got the rest
    // wrong in both directions — it claimed a `minimal` level no model has, and
    // capped everything else at `high`.
    expect(effortChoicesFor(catalog, "codex", "gpt-5.5")).toEqual([
      "low",
      "medium",
      "high",
      "xhigh",
    ]);
    expect(effortChoicesFor(catalog, "codex", "gpt-5.6-sol")).toContain("ultra");
    expect(effortChoicesFor(catalog, "codex", "gpt-5.6-sol")).not.toContain("minimal");
  });

  it("falls back to the harness-wide union with no model picked", () => {
    // "" is the un-chosen state. Offering the levels *some* model accepts beats
    // offering none; picking a model narrows it immediately.
    expect(effortChoicesFor(catalog, "codex", "")).toEqual([
      "low",
      "medium",
      "high",
      "xhigh",
      "max",
      "ultra",
    ]);
  });

  it("reports harness availability, treating an unloaded catalog as available", () => {
    expect(harnessAvailable(catalog, "claude")).toBe(true);
    expect(harnessAvailable(catalog, "codex")).toBe(false);
    // Not in the catalog at all — either not loaded yet or an unknown harness.
    // Hiding the picker on first paint would flicker; showing it is the safer
    // default because the spawn itself still refuses an unavailable source.
    expect(harnessAvailable([], "claude")).toBe(true);
  });
});

describe("agentToolsForSources", () => {
  const src = (name: string, on = true) => ({ name, enabled: on, available: on });

  // The launcher offered only Claude and Codex, so a machine with a working dsh
  // source could list dsh sessions but never start one.
  it("offers dsh once its source is enabled and available", () => {
    const tools = agentToolsForSources([src("claude-code"), src("dsh")]);
    expect(tools.map((t) => t.value)).toEqual(["claude", "dsh"]);
  });

  // dsh's source is additionally gated on the binary existing, so "not
  // available" is the normal state on a machine without dsh installed — it must
  // not show a tool that cannot launch.
  it("hides dsh when its source is disabled or the binary is missing", () => {
    expect(
      agentToolsForSources([src("claude-code"), src("dsh", false)]).map((t) => t.value),
    ).toEqual(["claude"]);
  });
});

describe("toolForAgentSource", () => {
  it("maps each launchable source onto its tool value", () => {
    expect(toolForAgentSource("claude-code")).toBe("claude");
    expect(toolForAgentSource("codex")).toBe("codex");
    // Was the bug: a hardcoded `=== "codex" ? "codex" : "claude"` made the
    // resume and schedule editors offer Claude's model list for a dsh session.
    expect(toolForAgentSource("dsh")).toBe("dsh");
  });

  it("falls back to claude for anything Fleet cannot launch", () => {
    expect(toolForAgentSource("some-future-agent")).toBe("claude");
    expect(toolForAgentSource("")).toBe("claude");
    expect(toolForAgentSource(undefined)).toBe("claude");
    expect(toolForAgentSource(null)).toBe("claude");
  });
});

describe("tokenPanelForAgentSource", () => {
  it("gives dsh its own panel instead of the file-reading Claude one", () => {
    // Was the bug: the Token tab's `agentSource === "codex" ? … : …` ternary
    // sent dsh to `TokenSpendPanel`, which reads the session's JSONL — dsh has
    // no file (`resolve_file_path` returns None), so the tab rendered nothing.
    expect(tokenPanelForAgentSource("dsh")).toBe("dsh");
  });

  it("leaves claude and codex on the panels they already had", () => {
    expect(tokenPanelForAgentSource("codex")).toBe("codex");
    expect(tokenPanelForAgentSource("claude-code")).toBe("claude");
    expect(tokenPanelForAgentSource("")).toBe("claude");
    expect(tokenPanelForAgentSource(undefined)).toBe("claude");
    expect(tokenPanelForAgentSource(null)).toBe("claude");
  });
});
