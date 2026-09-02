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
/** 单设备时代的键。只在首次读取时被认领一次(见 loadCachedSessions),之后删除。 */
const LEGACY_KEY = "sessions-snapshot-v1";

/** 每台设备各自一份快照。多设备之前只有一个全局键,于是切换设备会先画出另一台
 *  的任务列表 —— 那不只是不好看:列表里的会话 id 只在单机内唯一,点进去就是一次
 *  张冠李戴的请求。 */
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
  // 首次升级:全局键里那份属于当时唯一在册的那台设备。认领它(而不是丢掉)是为了
  // 保住冷启动那一眼即时渲染;认领后删掉全局键,免得第二台设备也来认领同一份。
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

/** Drop一台设备的快照。解除配对/移除设备时调用,免得下一次配对(可能是另一个
 *  账号)先短暂画出上一台的任务列表。`deviceId` 为 null 时清的是遗留全局键。 */
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
