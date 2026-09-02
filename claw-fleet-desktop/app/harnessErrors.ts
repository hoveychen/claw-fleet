// Classify raw harness-spawn error strings into the harness they belong to,
// so error surfaces can render a localized message plus a "fix it in the
// environment panel" action instead of leaking the backend string.
//
// These patterns intentionally match the *current* backend messages
// (session_launch.rs / codex_launch.rs / codex_source.rs / dsh_source.rs);
// they are a display-layer nicety — an unmatched message still renders raw, so
// a backend wording change degrades gracefully rather than hiding the error.

export type HarnessSource = "claude-code" | "codex" | "dsh";

const PATTERNS: Array<[RegExp, HarnessSource]> = [
  [/claude (cli|binary) not found/i, "claude-code"],
  [/codex (cli|binary) not found/i, "codex"],
  [/dsh is not installed/i, "dsh"],
];

export function classifyHarnessError(message: string): HarnessSource | null {
  for (const [re, source] of PATTERNS) {
    if (re.test(message)) return source;
  }
  return null;
}

/** Transient cross-window signal: which settings tab to land on. Read and
 * consumed by SettingsPanel (mount + `storage` event for an already-open
 * window). Plain localStorage on purpose — it must not persist restarts, so
 * it stays out of storage.ts's ALL_KEYS. */
export const SETTINGS_OPEN_TAB_KEY = "settings-open-tab";

export function requestSettingsTab(tab: string) {
  try {
    window.localStorage.setItem(SETTINGS_OPEN_TAB_KEY, tab);
  } catch {
    /* private mode etc. — the settings window just opens on its default tab */
  }
}
