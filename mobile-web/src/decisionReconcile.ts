import type { AgentFingerprint, PendingDecision } from "./types";

/** Collapse a fingerprint into the string reconcile compares for equality.
 *  `undefined` when the reply carried no `agent` at all — that's the
 *  "unattributable" case the cooldown below covers. */
export function agentKeyOf(agent: AgentFingerprint | undefined): string | undefined {
  if (!agent) return undefined;
  const { host = "?", pid = 0, home = "?", ver = "?" } = agent;
  return `${host}/pid=${pid}/home=${home}/v${ver}`;
}

/** How long to keep suppressing a card the user answered on THIS device but
 *  whose answer hasn't been confirmed processed by the desktop yet. The happy
 *  path clears far sooner — the moment a snapshot no longer lists the card
 *  (positive confirmation). This timer is only the fallback for a genuinely
 *  lost answer frame (fire-and-forget send dropped on a flaky link): after it
 *  elapses the card is allowed back so Boss can re-answer. */
export const ANSWER_GRACE_MS = 30_000;

/** How long a just-arrived card is shielded from an *unattributable* empty
 *  snapshot (one whose reply carries no `agent` fingerprint — an older desktop,
 *  or a stray agent built before fingerprinting). The relay broadcasts every
 *  request to every agent in the channel and the phone takes whichever reply
 *  lands first, so an empty list can come from an agent that simply cannot see
 *  this machine's `~/.fleet`. Past this window an empty snapshot is believed
 *  again, so a genuinely resolved card can never be pinned on screen for long. */
export const EMPTY_SNAPSHOT_COOLDOWN_MS = 20_000;

/** Reconcile the authoritative `pending_snapshot` against the locally-held
 *  card list, honouring cards the user just answered on THIS device whose
 *  answer is still in flight. */
export interface ReconcileInput {
  /** The list currently rendered (used to preserve each card's arrival stamp
   *  so a refresh doesn't reshuffle the queue). */
  prev: PendingDecision[];
  /** The authoritative snapshot pulled from the desktop. */
  fresh: PendingDecision[];
  /** id → timestamp of a local answer still awaiting desktop confirmation. */
  answeredAt: Map<string, number>;
  /** Current wall clock (injected for testability). */
  now: number;
  /** Fingerprint of the agent that served *this* snapshot (`agent` field of the
   *  reply). `undefined` when the desktop predates fingerprinting. */
  agentKey?: string;
  /** Fingerprint of the agent the phone currently treats as its desktop — the
   *  source of the last snapshot that actually carried cards. */
  trustedAgentKey?: string;
}

export interface ReconcileResult {
  decisions: PendingDecision[];
  /** Pruned copy of `answeredAt` (confirmed-landed and grace-expired ids removed). */
  answeredAt: Map<string, number>;
  /** Set when the whole snapshot was discarded as untrustworthy: the caller
   *  keeps `prev` on screen and records the source for diagnosis. */
  ignored?: { reason: "foreign-empty-snapshot" | "fresh-card-cooldown"; agentKey?: string };
  /** Fingerprint to remember as the trusted agent after this reconcile. */
  trustedAgentKey?: string;
}

export function reconcileDecisions({
  prev,
  fresh,
  answeredAt,
  now,
  agentKey,
  trustedAgentKey,
}: ReconcileInput): ReconcileResult {
  // ── Untrustworthy-empty guard ─────────────────────────────────────────────
  // Only "the snapshot says nothing is pending while we are showing something"
  // is ever second-guessed. A snapshot that drops *some* cards is ordinary
  // (they were answered), and one that adds cards can only come from an agent
  // that sees real requests — neither is affected.
  if (fresh.length === 0 && prev.length > 0) {
    const attributable = !!agentKey && !!trustedAgentKey;
    if (attributable && agentKey !== trustedAgentKey) {
      // A different agent than the one that has been feeding us cards. It is
      // answering out of a filesystem that isn't this desktop's, so its "empty"
      // says nothing about our cards. Keep them, and keep trusting the old key —
      // adopting the intruder's would flip the roles on the next snapshot.
      return {
        decisions: prev,
        answeredAt,
        ignored: { reason: "foreign-empty-snapshot", agentKey },
        trustedAgentKey,
      };
    }
    if (!attributable && prev.some((d) => now - d.arrivedAt < EMPTY_SNAPSHOT_COOLDOWN_MS)) {
      // Can't tell who answered (no fingerprint on either side), so fall back to
      // age: a card that arrived seconds ago being "already gone" is far more
      // likely a stray agent than a real resolution. Bounded by the cooldown.
      return {
        decisions: prev,
        answeredAt,
        ignored: { reason: "fresh-card-cooldown", agentKey },
        trustedAgentKey,
      };
    }
  }
  const freshIds = new Set(fresh.map((d) => d.id));
  // Prune the in-flight-answer set: an id the desktop has dropped from the
  // snapshot is confirmed processed (clear it); an id still listed past the
  // grace window is a presumed-lost answer (clear it so the card can return).
  const nextAnswered = new Map<string, number>();
  for (const [id, at] of answeredAt) {
    if (freshIds.has(id) && now - at < ANSWER_GRACE_MS) nextAnswered.set(id, at);
  }
  const seen = new Map(prev.map((d) => [d.id, d.arrivedAt]));
  const decisions = fresh
    // Hide cards whose local answer is still in flight — the snapshot lags the
    // just-sent answer on a slow link, and without this the card flickers back.
    .filter((d) => !nextAnswered.has(d.id))
    .map((d) => ({ ...d, arrivedAt: seen.get(d.id) ?? d.arrivedAt }));
  // A snapshot that carried cards proves its sender can see this machine's
  // pending requests — that's the agent to trust from here on. An empty one we
  // *did* believe leaves the old key in place: there is nothing to promote.
  const nextTrusted = fresh.length > 0 ? (agentKey ?? trustedAgentKey) : trustedAgentKey;
  return { decisions, answeredAt: nextAnswered, trustedAgentKey: nextTrusted };
}
