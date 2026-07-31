// Shared catalog of selectable Claude models. Consumers prepend their own
// "default" entry, since the default differs per surface (the new-session
// launcher, for one, follows the CLI's own configured model).
export const CLAUDE_MODEL_CHOICES: { value: string; label: string }[] = [
  { value: "claude-fable-5", label: "Fable 5" },
  { value: "claude-opus-5", label: "Opus 5" },
  { value: "claude-opus-4-8", label: "Opus 4.8" },
  { value: "claude-sonnet-5", label: "Sonnet 5" },
  { value: "claude-sonnet-4-6", label: "Sonnet 4.6" },
  { value: "claude-haiku-4-5-20251001", label: "Haiku 4.5" },
];

// `claude --effort <level>` accepted values (verified against `claude --help`).
export const CLAUDE_EFFORT_CHOICES: string[] = ["low", "medium", "high", "xhigh", "max"];

// Selectable Codex models (`codex exec -m <model>`), curated from the ids
// Codex ships (see `~/.codex/models_cache.json`). The "" default follows
// Codex's own configured model. Kept small on purpose — add ids as needed.
//
// A `<provider>:` prefix routes the run at a custom `[model_providers.<id>]`
// block in `~/.codex/config.toml` (Codex needs `-c model_provider=<id>` on top
// of `-m <model>`; the config block alone only defines the provider). The
// backend splits the pair — see `split_model_provider` in `codex_launch.rs`.
// Prefixed entries only work once the user has written that block themselves;
// picking one without it makes Codex fail fast on an unknown provider id.
export const CODEX_MODEL_CHOICES: { value: string; label: string }[] = [
  { value: "gpt-5.6-sol", label: "GPT-5.6 Sol" },
  { value: "gpt-5.6-terra", label: "GPT-5.6 Terra" },
  { value: "gpt-5.6-luna", label: "GPT-5.6 Luna" },
  { value: "gpt-5.5", label: "GPT-5.5" },
  {
    value: "openrouter:deepseek/deepseek-v4-flash",
    label: "DeepSeek V4 Flash (OpenRouter)",
  },
  {
    value: "openrouter:deepseek/deepseek-v4-pro",
    label: "DeepSeek V4 Pro (OpenRouter)",
  },
];

// Codex reasoning effort (`-c model_reasoning_effort=<level>`). Distinct from
// Claude's --effort scale (no "xhigh"/"max"); Codex adds "minimal".
export const CODEX_EFFORT_CHOICES: string[] = ["minimal", "low", "medium", "high"];

// The agent tools Fleet can launch a new session with.
export const AGENT_TOOL_CHOICES: { value: string; label: string }[] = [
  { value: "claude", label: "Claude" },
  { value: "codex", label: "Codex" },
];

/** A source entry as returned by the `get_sources_config` backend command. */
export interface SourceInfo {
  name: string;
  enabled: boolean;
  available: boolean;
}

/** Map an agent-source name (as used by the backend registry) to the launcher's
 *  tool value. The Claude source is registered under "claude-code" but the
 *  launcher tool value is the bare "claude". */
function sourceNameToTool(name: string): string {
  return name === "claude-code" ? "claude" : name;
}

/** Restrict the launchable agent tools to the sources that are actually being
 *  monitored — a source must be both enabled (the settings toggle) and available
 *  (installed). This is what keeps Codex out of the launcher when its source is
 *  turned off or the CLI isn't installed. Falls back to Claude-only when the
 *  config hasn't loaded yet or nothing matched, so the launcher is never empty
 *  and never flashes an unmonitored tool before hiding it. */
export function agentToolsForSources(
  sources: SourceInfo[],
): { value: string; label: string }[] {
  const active = new Set(
    sources.filter((s) => s.enabled && s.available).map((s) => sourceNameToTool(s.name)),
  );
  const filtered = AGENT_TOOL_CHOICES.filter((c) => active.has(c.value));
  return filtered.length ? filtered : [AGENT_TOOL_CHOICES[0]];
}

// `claude --permission-mode <mode>` values exposed in the new-session
// launcher (a curated subset of the CLI's choices — "auto"/"dontAsk" are
// omitted as they mostly matter for interactive runs). Labels come from the
// `new_session.permission_*` locale keys.
export const CLAUDE_PERMISSION_MODE_CHOICES: string[] = [
  "acceptEdits",
  "plan",
  "bypassPermissions",
];
