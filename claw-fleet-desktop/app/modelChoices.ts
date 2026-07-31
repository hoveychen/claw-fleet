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
// Third-party models are NOT listed here: they come from the host's Codex
// profile files at runtime (see `codexProfileChoices`). Hardcoding them would
// offer models whose provider block may not exist on the machine running Codex.
export const CODEX_MODEL_CHOICES: { value: string; label: string }[] = [
  { value: "gpt-5.6-sol", label: "GPT-5.6 Sol" },
  { value: "gpt-5.6-terra", label: "GPT-5.6 Terra" },
  { value: "gpt-5.6-luna", label: "GPT-5.6 Luna" },
  { value: "gpt-5.5", label: "GPT-5.5" },
];

/** A Codex profile-v2 file as returned by the `list_codex_profiles` command —
 *  one `<CODEX_HOME>/<name>.config.toml` on whichever host runs Codex. */
export interface CodexProfile {
  name: string;
  model: string | null;
  model_provider: string | null;
  reasoning_effort: string | null;
}

/** Turn discovered profiles into model-picker entries.
 *
 *  A `[model_providers.<id>]` block declares only how to reach a provider, not
 *  which models it serves, so a profile file is the only thing in Codex's
 *  config that names a usable third-party model. One profile = one entry.
 *
 *  The value carries the `profile:` marker the backend splits into
 *  `codex exec -p <name>` (see `push_model_args` in `codex_launch.rs`); `-p`
 *  brings the profile's model *and* provider, which is why the raw model id is
 *  never sent for these. The label prefers the profile's own model id — that is
 *  what the user recognises — and falls back to the profile name for a profile
 *  that sets no model of its own. Effort stays selectable regardless: Fleet's
 *  `-c model_reasoning_effort=` flag overrides whatever the profile sets. */
export function codexProfileChoices(
  profiles: CodexProfile[],
): { value: string; label: string }[] {
  return profiles.map((p) => {
    const model = p.model?.trim();
    const provider = p.model_provider?.trim();
    const label = model
      ? provider
        ? `${model} (${provider})`
        : model
      : p.name;
    return { value: `profile:${p.name}`, label };
  });
}

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
