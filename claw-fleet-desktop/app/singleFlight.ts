/**
 * Collapse concurrent calls of the same backend read into one in-flight request.
 *
 * # Why
 *
 * Several pollers here fire on a timer *and* on every `sessions-updated` push,
 * with no guard against the previous call still being out. That is fine while
 * the backend answers in milliseconds and catastrophic the moment it doesn't:
 * on a cold 2026-09-10 launch (1361 sessions) `today_usage` blocked on the
 * usage-cache lock for ~40s, the event storm kept firing, and **11** copies of
 * it piled up. Each one occupies a thread in Tauri's async runtime, which has
 * exactly `num_cpus` (10 here) of them — so the pile-up starved *every* other
 * `(async)` command, including `get_messages_tail` (task detail stuck on
 * 「加载中…」) and Tauri's own `plugin:event|listen`.
 *
 * Fixing the slow call (see `warm_usage_cache`) removes that particular stall;
 * this removes the amplifier, so the next slow call costs one worker instead of
 * eleven.
 *
 * # Semantics
 *
 * While a call for `key` is out, later callers get the **same** promise rather
 * than a fresh request — they see the in-flight result, not a newer one. That
 * is what a poller wants (the next tick fetches fresh anyway) and is wrong for
 * a read that must observe a write you just made; don't route those through
 * here. Rejections propagate to every caller, and the key is released either
 * way, so one failure doesn't wedge the key.
 */
const inFlight = new Map<string, Promise<unknown>>();

export function singleFlight<T>(key: string, run: () => Promise<T>): Promise<T> {
  const existing = inFlight.get(key) as Promise<T> | undefined;
  if (existing) return existing;

  const pending = run();
  inFlight.set(key, pending);
  // Release on both settle paths. Registering the handler here (rather than
  // returning `pending.finally(...)`) means every caller still receives the
  // original promise, and the rejection is considered handled by this listener
  // even if a caller only uses `.then`.
  void pending.then(
    () => inFlight.delete(key),
    () => inFlight.delete(key),
  );
  return pending;
}

/** Test seam: forget every in-flight key. Never needed in app code. */
export function resetSingleFlight(): void {
  inFlight.clear();
}
