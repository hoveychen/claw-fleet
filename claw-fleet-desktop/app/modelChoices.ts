// Shared catalog of selectable Claude models. Consumers prepend their own
// "default" entry, since the default differs per surface (the new-session
// launcher, for one, follows the CLI's own configured model).
export const CLAUDE_MODEL_CHOICES: { value: string; label: string }[] = [
  { value: "claude-fable-5", label: "Fable 5" },
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
export const CODEX_MODEL_CHOICES: { value: string; label: string }[] = [
  { value: "gpt-5.6-sol", label: "GPT-5.6 Sol" },
  { value: "gpt-5.6-terra", label: "GPT-5.6 Terra" },
  { value: "gpt-5.6-luna", label: "GPT-5.6 Luna" },
  { value: "gpt-5.5", label: "GPT-5.5" },
];

// Codex reasoning effort (`-c model_reasoning_effort=<level>`). Distinct from
// Claude's --effort scale (no "xhigh"/"max"); Codex adds "minimal".
export const CODEX_EFFORT_CHOICES: string[] = ["minimal", "low", "medium", "high"];

// The agent tools Fleet can launch a new session with.
export const AGENT_TOOL_CHOICES: { value: string; label: string }[] = [
  { value: "claude", label: "Claude" },
  { value: "codex", label: "Codex" },
];

// `claude --permission-mode <mode>` values exposed in the new-session
// launcher (a curated subset of the CLI's choices — "auto"/"dontAsk" are
// omitted as they mostly matter for interactive runs). Labels come from the
// `new_session.permission_*` locale keys.
export const CLAUDE_PERMISSION_MODE_CHOICES: string[] = [
  "acceptEdits",
  "plan",
  "bypassPermissions",
];
