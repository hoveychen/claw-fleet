/// Detecting the synthetic "skill body" turn Claude Code injects when a `Skill`
/// tool loads — the native port of the desktop / mobile-web `skillInjection.ts`.
///
/// A skill load emits a user text record (tagged `isMeta: true`, with a
/// `sourceToolUseID` back to the tool call) whose content is the entire
/// SKILL.md. That is the harness feeding instructions to the agent, not a user
/// turn, so we fold it into a collapsed card. The body always begins with this
/// exact prefix, emitted by Claude Code's skill loader.

export const SKILL_INJECTION_PREFIX: string = 'Base directory for this skill:';

export interface SkillInjection {
  slug: string;
  body: string;
}

export function parseSkillInjection(text: string): SkillInjection | null {
  if (!text.startsWith(SKILL_INJECTION_PREFIX)) {
    return null;
  }
  const nl: number = text.indexOf('\n');
  const firstLine: string = nl === -1 ? text : text.slice(0, nl);
  const dir: string = firstLine.slice(SKILL_INJECTION_PREFIX.length).trim();
  const parts: string[] = dir.split('/').filter((s: string) => s.length > 0);
  const slug: string = parts.length > 0 ? parts[parts.length - 1] : 'skill';
  return { slug, body: text };
}
