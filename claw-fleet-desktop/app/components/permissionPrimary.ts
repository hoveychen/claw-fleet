// The single field that matters when approving a native Claude Code permission
// prompt — the actual command / path / pattern / url the AI wants to run — so
// the card leads with what 老板 is deciding on instead of a raw JSON dump.
// Shared by the desktop PermissionPromptCard (and mirrored in mobile-web).

// Tools whose primary input is a filesystem path — surfaced with an extension
// glyph + basename headline (full path below) instead of a JSON dump.
export const PERM_FILE_TOOLS = new Set([
  "Read",
  "Edit",
  "Write",
  "MultiEdit",
  "NotebookEdit",
]);

export type PermPrimary =
  | { kind: "command"; text: string }
  | { kind: "file"; path: string }
  | { kind: "pattern"; text: string; path?: string }
  | { kind: "url"; text: string }
  | { kind: "json"; text: string };

/**
 * Pick the one field that matters for approving `toolName`. This is a security
 * gate, so we surface the real actionable value (Bash's `command`, not its
 * `description`). Unknown tools fall back to pretty-printed JSON. Field names
 * mirror Claude Code's native tool schema (Bash→command, Read/Edit/Write→
 * file_path, Grep/Glob→pattern, WebFetch→url, WebSearch→query).
 */
export function permissionPrimary(
  toolName: string,
  input: Record<string, unknown>,
): PermPrimary {
  const str = (v: unknown) => (typeof v === "string" ? v : "");
  if (PERM_FILE_TOOLS.has(toolName) && str(input.file_path)) {
    return { kind: "file", path: str(input.file_path) };
  }
  if ((toolName === "Grep" || toolName === "Glob") && str(input.pattern)) {
    return { kind: "pattern", text: str(input.pattern), path: str(input.path) || undefined };
  }
  if (toolName === "WebFetch" && str(input.url)) return { kind: "url", text: str(input.url) };
  if (toolName === "WebSearch" && str(input.query)) return { kind: "url", text: str(input.query) };
  // Generic field fallbacks for any other tool.
  if (str(input.command)) return { kind: "command", text: str(input.command) };
  if (str(input.cmd)) return { kind: "command", text: str(input.cmd) };
  if (str(input.file_path)) return { kind: "file", path: str(input.file_path) };
  if (str(input.path)) return { kind: "file", path: str(input.path) };
  if (str(input.pattern)) return { kind: "pattern", text: str(input.pattern) };
  if (str(input.url)) return { kind: "url", text: str(input.url) };
  if (str(input.query)) return { kind: "url", text: str(input.query) };
  try {
    return { kind: "json", text: JSON.stringify(input, null, 2) };
  } catch {
    return { kind: "json", text: String(input) };
  }
}
