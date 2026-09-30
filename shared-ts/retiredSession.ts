/**
 * A manual resume of a session another one has taken over (a handoff or a
 * plan revive) is refused once by the backend with an error naming the
 * successor — `agent_source::guard_manual_resume` on the Rust side. Both apps
 * ask the boss and resend with `allowRetired: true`; this reads that error.
 *
 * On 2026-09-29 the boss batch-continued a session next to the revive that had
 * replaced it, and two agents ran the same plan.
 */
const RETIRED_RE = /retired: session \S+ was taken over by (\S+)/;

/** The successor's session id when `err` is the retired refusal, else `null`. */
export function retiredSuccessor(err: unknown): string | null {
  const text =
    typeof err === "string" ? err : String((err as { message?: unknown } | null)?.message ?? err);
  return RETIRED_RE.exec(text)?.[1] ?? null;
}
