import { describe, expect, it } from "vitest";
import {
  CODEX_MODEL_CHOICES,
  codexProfileChoices,
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

  it("keeps third-party models out of the built-in catalog", () => {
    // The built-ins are Codex's own ids; anything provider-specific must arrive
    // at runtime from the host's profile files, never hardcoded here.
    for (const c of CODEX_MODEL_CHOICES) {
      expect(c.value.startsWith("profile:")).toBe(false);
      expect(c.value).not.toContain("/");
    }
  });
});
