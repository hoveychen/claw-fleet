import { describe, expect, it } from "vitest";
import {
  CODEX_MODEL_CHOICES,
  agentToolsForSources,
  codexProfileChoices,
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
  it("offers GPT-6 Astra in the built-in catalog", () => {
    expect(CODEX_MODEL_CHOICES).toContainEqual({
      value: "gpt-6-astra",
      label: "GPT-6 Astra",
    });
  });

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

  it("keeps third-party models out of the built-in catalog", () => {
    // The built-ins are Codex's own ids; anything provider-specific must arrive
    // at runtime from the host's profile files, never hardcoded here.
    for (const c of CODEX_MODEL_CHOICES) {
      expect(c.value.startsWith("profile:")).toBe(false);
      expect(c.value).not.toContain("/");
    }
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
