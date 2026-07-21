// Pairing-secret persistence. localStorage alone is fragile on iOS Safari:
// a site not added to the home screen gets its script-writable storage wiped
// after 7 days without a visit, and "clear website data" nukes it instantly.
// We mirror the secret into IndexedDB as a second copy (evicted independently
// in some cleanup paths) and re-hydrate from whichever copy survived. The
// real durability fix is installing as a PWA — see the A2HS hint in App.

const LS_KEY = "fleet-relay-secret";
const DB_NAME = "fleet-relay";
const STORE = "kv";

/** Shared by sessionCache.ts, which stores the sessions snapshot under a
 *  different key in this same DB/store (no schema change). */
export function openDb(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open(DB_NAME, 1);
    req.onupgradeneeded = () => {
      if (!req.result.objectStoreNames.contains(STORE)) {
        req.result.createObjectStore(STORE);
      }
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

/** Synchronous sources only: URL fragment (`#k=…`) wins, then localStorage.
 *  A fragment hit is persisted to both stores and scrubbed from the URL. */
export function loadSecretSync(): string | null {
  const match = window.location.hash.match(/[#&]k=([A-Za-z0-9_-]+)/);
  if (match) {
    persistSecret(match[1]);
    history.replaceState(null, "", window.location.pathname);
    return match[1];
  }
  return localStorage.getItem(LS_KEY);
}

/** IndexedDB fallback for when localStorage was wiped but IDB survived. */
export async function loadSecretFromIdb(): Promise<string | null> {
  try {
    const db = await openDb();
    return await new Promise<string | null>((resolve) => {
      const tx = db.transaction(STORE, "readonly");
      const req = tx.objectStore(STORE).get(LS_KEY);
      req.onsuccess = () => resolve(typeof req.result === "string" ? req.result : null);
      req.onerror = () => resolve(null);
    });
  } catch {
    return null;
  }
}

/** Write-through to both stores (IDB write is fire-and-forget). */
export function persistSecret(secret: string): void {
  try {
    localStorage.setItem(LS_KEY, secret);
  } catch {
    // storage full / private mode — IDB below may still work
  }
  void openDb()
    .then(
      (db) =>
        new Promise<void>((resolve) => {
          const tx = db.transaction(STORE, "readwrite");
          tx.objectStore(STORE).put(secret, LS_KEY);
          tx.oncomplete = () => resolve();
          tx.onerror = () => resolve();
        }),
    )
    .catch(() => {});
}

export function clearSecret(): void {
  try {
    localStorage.removeItem(LS_KEY);
  } catch {
    // ignore
  }
  void openDb()
    .then(
      (db) =>
        new Promise<void>((resolve) => {
          const tx = db.transaction(STORE, "readwrite");
          tx.objectStore(STORE).delete(LS_KEY);
          tx.oncomplete = () => resolve();
          tx.onerror = () => resolve();
        }),
    )
    .catch(() => {});
}

/** iOS Safari running as a plain tab (not installed to the home screen) —
 *  the case where the 7-day storage eviction applies. */
export function needsA2hsForDurableStorage(): boolean {
  const isIos = /iphone|ipad|ipod/i.test(navigator.userAgent);
  const standalone =
    (navigator as { standalone?: boolean }).standalone === true ||
    window.matchMedia("(display-mode: standalone)").matches;
  return isIos && !standalone;
}
