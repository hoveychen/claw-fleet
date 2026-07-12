import type { SessionInfo } from "./types";

/** Grace-window confirmation for a spawned session whose `spawn_session` reply
 *  was lost or arrived late.
 *
 *  The relay forwards frames best-effort to whatever client is connected at
 *  send time — no queue, no retry (see fleet-relay registry::forward). A phone
 *  that switched networks / slept / reconnected between sending the request and
 *  the reply never sees the reply, so `client.request("spawn_session")` rejects
 *  with a timeout even though the desktop already spawned the session.
 *
 *  Because the phone now pre-assigns the session id (passed as `session_id`),
 *  the desktop launches `claude --session-id <that id>`; the scanner discovers
 *  the JSONL whose stem equals the id and pushes it in a `sessions` snapshot.
 *  So we can confirm success independently of the lost reply by watching the
 *  snapshot for a session whose `id` matches.
 *
 *  `getSessions` is re-read every poll so it always sees the latest snapshot.
 *  Timers are injected so the logic is unit-testable without real delays. */
export async function waitForSessionId(
  sessionId: string,
  getSessions: () => SessionInfo[],
  opts: {
    attempts?: number;
    intervalMs?: number;
    sleep?: (ms: number) => Promise<void>;
  } = {},
): Promise<boolean> {
  const attempts = opts.attempts ?? 25; // 25 × 800ms ≈ 20s grace window
  const intervalMs = opts.intervalMs ?? 800;
  const sleep = opts.sleep ?? ((ms) => new Promise((r) => setTimeout(r, ms)));
  const present = () => getSessions().some((s) => s.id === sessionId);
  if (present()) return true;
  for (let i = 0; i < attempts; i++) {
    await sleep(intervalMs);
    if (present()) return true;
  }
  return false;
}
