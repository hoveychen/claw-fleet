// Persistent cache of the last sessions snapshot. Without it the task list lives
// only in React state + the FleetTransport's in-memory snapshot, so a cold start —
// or iOS evicting the backgrounded PWA — blanks the list until the desktop
// pushes a fresh full snapshot over the socket. Caching the last list in
// IndexedDB lets the app paint it instantly on boot, then reconcile live (Gmail-
// style: render stale, then refresh).
//
// Reuses secretStore's IndexedDB store ("fleet-relay" / "kv") via a distinct
// key, so there's no schema/version change — only a new value alongside the
// pairing secret. The store is structured-clone, so the SessionInfo[] is stored
// as-is (no JSON string round-trip).
import type { SessionInfo } from "./types";
import { openDb } from "./secretStore";

const STORE = "kv";
/** Key from the single-device era. Claimed once on first read (see loadCachedSessions),
 *  then deleted. */
const LEGACY_KEY = "sessions-snapshot-v1";

/** Each device gets its own snapshot. Before multi-device, there was only one global key,
 *  so switching devices would paint the other device's task list first — a problem that's
 *  not just cosmetic: session IDs are only unique per machine, so clicking in would
 *  send a wrong-session request. */
function keyFor(deviceId: string | null): string {
  return deviceId ? `sessions-snapshot-v2:${deviceId}` : LEGACY_KEY;
}
/** Ceiling on cached rows so the entry can't grow unbounded. Mirrors the
 *  desktop's SNAPSHOT_MAX_SESSIONS (500) — the most it ever sends anyway. */
const MAX_CACHED = 500;

/** Last cached sessions snapshot, or `null` if none / unreadable. Best-effort:
 *  any IndexedDB failure resolves to `null` so boot never blocks on the cache. */
export async function loadCachedSessions(
  deviceId: string | null,
): Promise<SessionInfo[] | null> {
  const own = await readCached(keyFor(deviceId));
  if (own || !deviceId) return own;
  // First upgrade: the global key held the one registered device at that time. Claim it
  // (rather than discard) to preserve instant cold-start rendering; after claiming,
  // delete the global key so a second device doesn't also claim the same snapshot.
  const legacy = await readCached(LEGACY_KEY);
  if (!legacy) return null;
  saveCachedSessions(deviceId, legacy);
  dropKey(LEGACY_KEY);
  return legacy;
}

function readCached(key: string): Promise<SessionInfo[] | null> {
  return (async () => {
    const db = await openDb();
    return await new Promise<SessionInfo[] | null>((resolve) => {
      const tx = db.transaction(STORE, "readonly");
      const req = tx.objectStore(STORE).get(key);
      req.onsuccess = () => {
        const v = req.result;
        resolve(Array.isArray(v) ? (v as SessionInfo[]) : null);
      };
      req.onerror = () => resolve(null);
    });
  })().catch(() => null);
}

/** Write-through the latest full sessions list (fire-and-forget; a failed write
 *  just means the next cold start falls back to a live full snapshot). */
export function saveCachedSessions(deviceId: string | null, list: SessionInfo[]): void {
  const trimmed = list.length > MAX_CACHED ? list.slice(0, MAX_CACHED) : list;
  const key = keyFor(deviceId);
  void openDb()
    .then(
      (db) =>
        new Promise<void>((resolve) => {
          const tx = db.transaction(STORE, "readwrite");
          tx.objectStore(STORE).put(trimmed, key);
          tx.oncomplete = () => resolve();
          tx.onerror = () => resolve();
        }),
    )
    .catch(() => {});
}

/** Drop a device's snapshot. Called when unpairing/removing a device, so the next
 *  pairing (possibly with a different account) doesn't briefly show the previous device's
 *  task list. When `deviceId` is null, clears the legacy global key. */
export function clearCachedSessions(deviceId: string | null): void {
  dropKey(keyFor(deviceId));
}

function dropKey(key: string): void {
  void openDb()
    .then(
      (db) =>
        new Promise<void>((resolve) => {
          const tx = db.transaction(STORE, "readwrite");
          tx.objectStore(STORE).delete(key);
          tx.oncomplete = () => resolve();
          tx.onerror = () => resolve();
        }),
    )
    .catch(() => {});
}
