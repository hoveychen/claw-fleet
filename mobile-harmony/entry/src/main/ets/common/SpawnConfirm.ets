import { SessionInfo } from '../models/Types';

/// Grace-window confirmation for a spawned session whose `spawn_session` reply
/// (and early ack) were both lost — native port of mobile-web/src/spawnConfirm.ts.
/// Last-resort fallback after the ack-optimistic path and the idempotent resend:
/// the desktop may have spawned the session yet every frame back to the phone was
/// dropped (relay delivery is best-effort — no queue, no retry). Because the phone
/// pre-assigns the session id, the desktop launches `claude --session-id <id>` and
/// the scanner surfaces it in a later `sessions` snapshot, so success can be
/// confirmed by watching the snapshot for a matching `id`.
///
/// `getSessions` is re-read every poll so it always sees the latest snapshot.
/// `sleep` is injected so the logic is unit-testable without real delays.

export interface WaitForSessionOpts {
  attempts?: number;
  intervalMs?: number;
  sleep?: (ms: number) => Promise<void>;
}

const DEFAULT_ATTEMPTS = 25;   // 25 × 800ms ≈ 20s grace window
const DEFAULT_INTERVAL_MS = 800;

function defaultSleep(ms: number): Promise<void> {
  return new Promise<void>((resolve: () => void) => {
    setTimeout(() => resolve(), ms);
  });
}

export async function waitForSessionId(
  sessionId: string,
  getSessions: () => SessionInfo[],
  opts?: WaitForSessionOpts,
): Promise<boolean> {
  const o: WaitForSessionOpts = opts ?? {};
  const attempts = o.attempts ?? DEFAULT_ATTEMPTS;
  const intervalMs = o.intervalMs ?? DEFAULT_INTERVAL_MS;
  const sleep = o.sleep ?? defaultSleep;
  const present = (): boolean => getSessions().some((s: SessionInfo) => s.id === sessionId);
  if (present()) {
    return true;
  }
  for (let i = 0; i < attempts; i++) {
    await sleep(intervalMs);
    if (present()) {
      return true;
    }
  }
  return false;
}
