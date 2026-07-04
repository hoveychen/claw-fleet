// Shared catalog of selectable Claude models. Consumers prepend their own
// "default" entry (the default differs per surface: TaskComposer's planner
// defaults to Opus 4.8, the new-session launcher follows the CLI's own
// configured model).
export const CLAUDE_MODEL_CHOICES: { value: string; label: string }[] = [
  { value: "claude-fable-5", label: "Fable 5" },
  { value: "claude-opus-4-8", label: "Opus 4.8" },
  { value: "claude-sonnet-5", label: "Sonnet 5" },
  { value: "claude-sonnet-4-6", label: "Sonnet 4.6" },
  { value: "claude-haiku-4-5-20251001", label: "Haiku 4.5" },
];

// `claude --effort <level>` accepted values (verified against `claude --help`).
export const CLAUDE_EFFORT_CHOICES: string[] = ["low", "medium", "high", "xhigh", "max"];

// `claude --permission-mode <mode>` values exposed in the new-session
// launcher (a curated subset of the CLI's choices — "auto"/"dontAsk" are
// omitted as they mostly matter for interactive runs). Labels come from the
// `new_session.permission_*` locale keys.
export const CLAUDE_PERMISSION_MODE_CHOICES: string[] = [
  "acceptEdits",
  "plan",
  "bypassPermissions",
];
