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
const KEY = "sessions-snapshot-v1";
/** Ceiling on cached rows so the entry can't grow unbounded. Mirrors the
 *  desktop's SNAPSHOT_MAX_SESSIONS (500) — the most it ever sends anyway. */
const MAX_CACHED = 500;

/** Last cached sessions snapshot, or `null` if none / unreadable. Best-effort:
 *  any IndexedDB failure resolves to `null` so boot never blocks on the cache. */
export async function loadCachedSessions(): Promise<SessionInfo[] | null> {
  try {
    const db = await openDb();
    return await new Promise<SessionInfo[] | null>((resolve) => {
      const tx = db.transaction(STORE, "readonly");
      const req = tx.objectStore(STORE).get(KEY);
      req.onsuccess = () => {
        const v = req.result;
        resolve(Array.isArray(v) ? (v as SessionInfo[]) : null);
      };
      req.onerror = () => resolve(null);
    });
  } catch {
    return null;
  }
}

/** Write-through the latest full sessions list (fire-and-forget; a failed write
 *  just means the next cold start falls back to a live full snapshot). */
export function saveCachedSessions(list: SessionInfo[]): void {
  const trimmed = list.length > MAX_CACHED ? list.slice(0, MAX_CACHED) : list;
  void openDb()
    .then(
      (db) =>
        new Promise<void>((resolve) => {
          const tx = db.transaction(STORE, "readwrite");
          tx.objectStore(STORE).put(trimmed, KEY);
          tx.oncomplete = () => resolve();
          tx.onerror = () => resolve();
        }),
    )
    .catch(() => {});
}

/** Drop the cached snapshot. Call on unpair so a different pairing secret (a
 *  different account) can't briefly render the previous one's task list. */
export function clearCachedSessions(): void {
  void openDb()
    .then(
      (db) =>
        new Promise<void>((resolve) => {
          const tx = db.transaction(STORE, "readwrite");
          tx.objectStore(STORE).delete(KEY);
          tx.oncomplete = () => resolve();
          tx.onerror = () => resolve();
        }),
    )
    .catch(() => {});
}
