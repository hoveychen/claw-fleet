/**
 * Detecting the synthetic "skill body" turn Claude Code injects when a `Skill`
 * tool is invoked.
 *
 * Loading a skill produces THREE transcript records, not one:
 *   1. assistant `tool_use`  — `Skill { "skill": "game-pilot" }`
 *   2. user `tool_result`    — the short "Launching skill: game-pilot" ack
 *      (already rendered inside the Skill tool card)
 *   3. user text record      — the entire SKILL.md body, tagged `isMeta: true`
 *      with a `sourceToolUseID` pointing back at record 1.
 *
 * Record 3 is not something the user typed; it is the harness feeding the skill
 * instructions to the agent. Rendered as a normal user bubble it dumps the whole
 * skill file into the transcript (see the red-boxed blob老板 flagged). We fold it
 * into a collapsed card instead.
 *
 * The body always begins with this exact English prefix, emitted by Claude
 * Code's skill loader — verified byte-consistent across every skill-loading
 * transcript on disk. That prefix is the detection signal; `isMeta` is a bonus
 * guard the caller can add but not depend on (some transports may drop it).
 */
export const SKILL_INJECTION_PREFIX = "Base directory for this skill:";

export interface SkillInjection {
  /** The skill's slug — the last path segment of its base directory. */
  slug: string;
  /** The full injected body (markdown), including the base-directory line. */
  body: string;
}

/**
 * If `text` is a skill-body injection, returns its slug and body; otherwise null.
 */
export function parseSkillInjection(text: string): SkillInjection | null {
  if (!text.startsWith(SKILL_INJECTION_PREFIX)) return null;
  const nl = text.indexOf("\n");
  const firstLine = nl === -1 ? text : text.slice(0, nl);
  const dir = firstLine.slice(SKILL_INJECTION_PREFIX.length).trim();
  const slug = dir.split("/").filter(Boolean).pop() || "skill";
  return { slug, body: text };
}
