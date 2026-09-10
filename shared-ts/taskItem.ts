/** A TASKS.md P-task line, split into its `P<n>` marker and the rest.
 *
 *  Items are written `**P3** — do the thing`, so the P-number — the one part
 *  you scan a plan by — is also the part wrapped in the noisiest markup. Pull
 *  it out as a badge; the remainder is left with its markdown intact, because
 *  both renderers pass it through a real markdown render and its own emphasis
 *  and `code` spans are meant to come out as emphasis and code. Anything that
 *  doesn't match the marker shape keeps its text verbatim rather than being
 *  mangled by a half-baked markdown pass.
 *
 *  Shared by the desktop (任务 facet + 计划树 drawer, via `TaskLine`) and the
 *  phone (`PlanItemLine`) so a P-task reads the same on both. It had been
 *  hand-copied three times over, and the copies had already drifted: two
 *  stripped the emphasis markers instead of rendering them, and the desktop's
 *  own task panel used none of them and printed the asterisks raw. */
export function splitMarker(text: string): { marker: string | null; rest: string } {
  const m = /^\*\*(P\d+[a-z]?)\*\*\s*(?:[—–-]\s*)?([\s\S]*)$/.exec(text.trim());
  if (!m) return { marker: null, rest: text };
  return { marker: m[1], rest: m[2] };
}

/** A P-task flattened to one line of plain prose, for a `title` tooltip and for
 *  the 计划树's matrix cells. Markdown emphasis/code markers are dropped here
 *  because a tooltip renders nothing — it would only show the syntax. */
export function taskTip(text: string, max = 160): string {
  const { marker, rest } = splitMarker(text);
  const body = rest
    .replace(/`{1,3}/g, "")
    .replace(/\*\*/g, "")
    .replace(/\s+/g, " ")
    .trim();
  const head = marker ? `${marker} — ` : "";
  return head + (body.length > max ? `${body.slice(0, max)}…` : body);
}
